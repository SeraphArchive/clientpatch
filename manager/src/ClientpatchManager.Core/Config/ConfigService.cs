using Tomlyn.Parsing;
using Tomlyn.Syntax;

namespace ClientpatchManager.Core.Config;

/// <summary>
/// Comment-preserving TOML config service for clientpatch.toml.
///
/// Loading normalizes legacy switches through the shared schema validator. Editing
/// uses Tomlyn's syntax tree to preserve unrelated fields, comments, and section order
/// while writing canonical per-section enable switches.
/// </summary>
public sealed class ConfigService : IConfigService {
    public ClientpatchConfig Load(string tomlText) {
        Tomlyn.Model.TomlTable root;
        try { root = ConfigValidation.Parse(tomlText); }
        catch (Exception ex) when (ex is not FormatException) { throw new FormatException(ex.Message, ex); }
        Tomlyn.Model.TomlTable Table(string name) => ConfigValidation.GetTable(root, name) ?? new();
        string S(string section, string key, string fallback) => Table(section).TryGetValue(key, out var value) ? (string)value : fallback;
        bool B(string section, string key, bool fallback) => Table(section).TryGetValue(key, out var value) ? (bool)value : fallback;
        return new ClientpatchConfig {
            LoaderLog = S("loader", "log", Install.InstallLayout.LogStem),
            Lilypad = new(B("lilypad", "enable", false), S("lilypad", "api_base", ""), S("lilypad", "platform_base", ""), S("lilypad", "signing", "noop"), S("lilypad", "server_public_key_pem", "")) {
                Report = new(B("lilypad.report", "enable", true), S("lilypad.report", "url", ""))
            },
            Steam = new(B("steam", "enable", false), S("steam", "mode", "auto"), S("steam", "ip_country", "JP"), S("steam", "ui_language", "japanese")),
            Isolation = new(B("isolation", "enable", false), S("isolation", "suffix", "clientpatch")),
            Titlebar = new(B("titlebar", "enable", false), S("titlebar", "template", " — clientpatch → {api_host}")),
            BepInEx = root.ContainsKey("bepinex") ? new(B("bepinex", "enable", false), S("bepinex", "root", "BepInEx"), S("bepinex", "doorstop", "doorstop.dll")) : null,
            InteropDump = new(B("interopdump", "enable", false), S("interopdump", "out_dir", "interop"))
        };
    }
    public string ApplyToText(string tomlText, ClientpatchConfig edited) {
        var doc = SyntaxParser.Parse(tomlText.EndsWith('\n') ? tomlText : tomlText + "\n", "clientpatch.toml", validate: true);
        ThrowIfInvalid(doc);

        MigrateReport(doc);
        MigrateIsolation(doc);
        RemoveLegacyFields(doc.KeyValues, "");
        foreach (var table in doc.Tables) RemoveLegacyFields(table.Items, KeyText(table.Name));
        foreach (var section in Sections) {
            foreach (var key in Keys) {
                var desired = DesiredScalar(edited, section, key);
                if (desired is null || FindField(doc, section, key) is not { } field) continue;
                switch (field.Value) {
                    case StringValueSyntax { Token: { } token } value when value.Value != desired:
                        token.Text = QuoteString(desired); break;
                    case BooleanValueSyntax { Token: { } token } value when value.Value != (desired == "true"):
                        token.Text = desired;
                        token.TokenKind = desired == "true" ? TokenKind.True : TokenKind.False;
                        value.Value = desired == "true";
                        break;
                }
            }
        }
        // Never emit a document the proxy cannot parse: validate the result and refuse.
        AddMissingFields(doc, edited, Load(tomlText));
        var output = doc.ToString();
        _ = Load(output);
        return output;
    }

    public string RawValidate(string tomlText) {
        try { _ = Load(tomlText); return ""; }
        catch (FormatException ex) { return ex.Message; }
    }

    static readonly string[] Sections = { "lilypad", "steam", "isolation", "titlebar", "interopdump", "bepinex", "lilypad.report" };
    static readonly string[] Keys = { "enable", "api_base", "platform_base", "signing", "server_public_key_pem", "mode", "ip_country", "ui_language", "suffix", "template", "root", "doorstop", "out_dir", "url" };

    static void AddMissingFields(DocumentSyntax doc, ClientpatchConfig edited, ClientpatchConfig original) {
        foreach (var section in Sections) {
            foreach (var key in Keys) {
                var desired = DesiredScalar(edited, section, key);
                if (desired is null || FindField(doc, section, key) is not null) continue;
                if (key != "enable" && desired == DesiredScalar(original, section, key)) continue;
                AddField(doc, section, key, key == "enable" ? desired : QuoteString(desired));
            }
        }
    }

    static KeyValueSyntax? FindField(DocumentSyntax doc, string section, string key) {
        var table = doc.Tables.FirstOrDefault(t => KeyText(t.Name) == section);
        if (table is not null) return table.Items.FirstOrDefault(kv => KeyText(kv.Key) == key);
        if (DottedSection(doc, section) is { } dotted)
            return dotted.Items.FirstOrDefault(kv => KeyText(kv.Key) == dotted.Prefix + key);
        if (InlineSection(doc, section) is { } inline)
            return inline.Items.Select(item => item.KeyValue).FirstOrDefault(kv => kv is not null && KeyText(kv.Key) == key);
        return null;
    }

    static InlineTableSyntax? InlineSection(DocumentSyntax doc, string section) {
        if (doc.KeyValues.FirstOrDefault(kv => KeyText(kv.Key) == section)?.Value is InlineTableSyntax direct) return direct;
        var split = section.LastIndexOf('.');
        if (split < 0) return null;
        var parent = section[..split]; var child = section[(split + 1)..];
        if (InlineSection(doc, parent) is { } inline)
            return inline.Items.FirstOrDefault(item => KeyText(item.KeyValue?.Key) == child)?.KeyValue?.Value as InlineTableSyntax;
        return doc.Tables.FirstOrDefault(table => KeyText(table.Name) == parent)?.Items
            .FirstOrDefault(kv => KeyText(kv.Key) == child)?.Value as InlineTableSyntax;
    }

    static (SyntaxList<KeyValueSyntax> Items, string Prefix)? DottedSection(DocumentSyntax doc, string section) {
        if (doc.KeyValues.Any(kv => KeyText(kv.Key).StartsWith(section + ".", StringComparison.Ordinal)))
            return (doc.KeyValues, section + ".");
        foreach (var table in doc.Tables) {
            var parent = KeyText(table.Name);
            if (!section.StartsWith(parent + ".", StringComparison.Ordinal)) continue;
            var prefix = section[(parent.Length + 1)..] + ".";
            if (table.Items.Any(kv => KeyText(kv.Key).StartsWith(prefix, StringComparison.Ordinal))) return (table.Items, prefix);
        }
        return null;
    }

    static void MigrateReport(DocumentSyntax doc) {
        const string nested = "lilypad.report";
        var hasNested = doc.Tables.Any(table => KeyText(table.Name) == nested)
            || DottedSection(doc, nested) is not null || InlineSection(doc, nested) is not null;
        for (var i = doc.Tables.ChildrenCount - 1; i >= 0; i--) {
            var table = doc.Tables.ElementAt(i);
            if (KeyText(table.Name) != "report") continue;
            if (hasNested) { doc.Tables.RemoveChildAt(i); continue; }
            var header = SyntaxParser.Parse("[lilypad.report]\n", "clientpatch.toml", true).Tables.First();
            var name = header.Name; header.Name = null; table.Name = name;
        }
        for (var i = doc.KeyValues.ChildrenCount - 1; i >= 0; i--) {
            var field = doc.KeyValues.ElementAt(i); var key = KeyText(field.Key);
            if (key != "report" && !key.StartsWith("report.", StringComparison.Ordinal)) continue;
            if (hasNested) { doc.KeyValues.RemoveChildAt(i); continue; }
            var parsed = SyntaxParser.Parse($"lilypad.{key} = false\n", "clientpatch.toml", true).KeyValues.First();
            var name = parsed.Key; parsed.Key = null; field.Key = name;
        }
    }

    static void MigrateIsolation(DocumentSyntax doc) {
        const string old = "regredirect", current = "isolation";
        var hasCurrent = doc.Tables.Any(t => KeyText(t.Name) == current)
            || DottedSection(doc, current) is not null || InlineSection(doc, current) is not null;
        for (var i = doc.Tables.ChildrenCount - 1; i >= 0; i--) {
            var table = doc.Tables.ElementAt(i);
            if (KeyText(table.Name) != old) continue;
            if (hasCurrent) { doc.Tables.RemoveChildAt(i); continue; }
            var header = SyntaxParser.Parse("[isolation]\n", "clientpatch.toml", true).Tables.First();
            var name = header.Name; header.Name = null; table.Name = name;
        }
        for (var i = doc.KeyValues.ChildrenCount - 1; i >= 0; i--) {
            var field = doc.KeyValues.ElementAt(i); var key = KeyText(field.Key);
            if (key != old && !key.StartsWith(old + ".", StringComparison.Ordinal)) continue;
            if (hasCurrent) { doc.KeyValues.RemoveChildAt(i); continue; }
            var parsed = SyntaxParser.Parse($"{current}{key[old.Length..]} = false\n", "clientpatch.toml", true).KeyValues.First();
            var name = parsed.Key; parsed.Key = null; field.Key = name;
        }
    }

    static void AddField(DocumentSyntax doc, string section, string key, string literal) {
        var table = doc.Tables.FirstOrDefault(t => KeyText(t.Name) == section);
        if (table is null && DottedSection(doc, section) is { } dotted) {
            var fragment = SyntaxParser.Parse($"{dotted.Prefix}{key} = {literal}\n", "clientpatch.toml", true);
            var field = fragment.KeyValues.First(); fragment.KeyValues.RemoveChildAt(0); dotted.Items.Add(field); return;
        }
        var inline = table is null ? InlineSection(doc, section) : null;
        var split = section.LastIndexOf('.');
        var inlineParent = table is null && inline is null && split >= 0 ? InlineSection(doc, section[..split]) : null;
        if (inline is not null || inlineParent is not null) {
            // Parse a compact item so multiline table trivia does not leak into an inline table.
            var content = inline is not null ? $"{key} = {literal}" : $"{section[(split + 1)..]} = {{ {key} = {literal} }}";
            var compact = SyntaxParser.Parse($"x = {{ {content} }}\n", "clientpatch.toml", true);
            var holder = (InlineTableSyntax)compact.KeyValues.First().Value!;
            var entry = holder.Items.First(); holder.Items.RemoveChildAt(0);
            var destination = inline ?? inlineParent!;
            if (destination.Items.ChildrenCount > 0) destination.Items.Last().Comma = new SyntaxToken(TokenKind.Comma, ", ");
            destination.Items.Add(entry); return;
        }
        var parsed = SyntaxParser.Parse($"\n[{section}]\n\n{key} = {literal}\n", "clientpatch.toml", true);
        var newTable = parsed.Tables.First(); parsed.Tables.RemoveChildAt(0);
        if (table is null) doc.Tables.Add(newTable);
        else { var item = newTable.Items.First(); newTable.Items.RemoveChildAt(0); table.Items.Add(item); }
    }

    static void RemoveLegacyFields(SyntaxList<KeyValueSyntax> items, string section) {
        for (var index = items.ChildrenCount - 1; index >= 0; index--) {
            var field = items.ElementAt(index); var key = KeyText(field.Key);
            var full = section.Length == 0 ? key : section + "." + key;
            if (full == "loader.modules" || Sections.Any(name => full == name + ".enabled")) {
                items.RemoveChildAt(index); continue;
            }
            if (field.Value is InlineTableSyntax inline && (full == "loader" || Sections.Contains(full)))
                RemoveInlineLegacy(inline, full);
        }
    }

    static void RemoveInlineLegacy(InlineTableSyntax inline, string section) {
        for (var i = inline.Items.ChildrenCount - 1; i >= 0; i--) {
            var field = inline.Items.ElementAt(i).KeyValue;
            var key = KeyText(field?.Key);
            if ((section == "loader" && key == "modules") || (section != "loader" && key == "enabled")) {
                inline.Items.RemoveChildAt(i); continue;
            }
            if (field?.Value is InlineTableSyntax nested && Sections.Contains(section + "." + key))
                RemoveInlineLegacy(nested, section + "." + key);
        }
        if (inline.Items.ChildrenCount > 0) inline.Items.Last().Comma = null;
    }
    // --- helpers ---------------------------------------------------------

    /// <summary>The desired scalar value for a managed key; validation rejects invalid edits.</summary>
    private static string? DesiredScalar(ClientpatchConfig c, string section, string key) {
        if (key == "enable" && section is "lilypad" or "steam" or "isolation" or "titlebar" or "interopdump" or "bepinex" or "lilypad.report")
            return c.IsEnabled(section) ? "true" : "false";
        var value = (section, key) switch {
            ("lilypad", "api_base") => c.Lilypad.ApiBase,
            ("lilypad", "platform_base") => c.Lilypad.PlatformBase,
            ("lilypad", "signing") => c.Lilypad.Signing,
            ("lilypad", "server_public_key_pem") => c.Lilypad.PublicKeyPath,
            ("steam", "mode") => c.Steam.Mode,
            ("steam", "ip_country") => c.Steam.Country,
            ("steam", "ui_language") => c.Steam.UiLanguage,
            ("isolation", "suffix") => c.Isolation.Suffix,
            ("titlebar", "template") => c.Titlebar.Template,
            ("interopdump", "out_dir") => c.InteropDump?.OutDir ?? "interop",
            ("bepinex", "root") => c.BepInEx?.Root ?? "BepInEx",
            ("bepinex", "doorstop") => c.BepInEx?.Doorstop ?? "doorstop.dll",
            ("lilypad.report", "url") => c.Lilypad.Report.Url,
            _ => null,
        };

        return value;
    }

    private static void ThrowIfInvalid(DocumentSyntax doc) {
        if (doc.HasErrors)
            throw new FormatException(Diagnostics(doc));
    }

    private static string Diagnostics(DocumentSyntax doc) {
        var errors = doc.Diagnostics
            .Where(d => d.Kind == DiagnosticMessageKind.Error)
            .Select(d => d.Message);
        var joined = string.Join("; ", errors);
        return joined.Length > 0 ? joined : "invalid TOML";
    }

    private static string KeyText(SyntaxNode? node) => node switch {
        KeySyntax key => string.Join(".", new[] { KeyText(key.Key) }.Concat(key.DotKeys.Select(item => KeyText(item.Key)))),
        BareKeySyntax key => key.Key?.Text ?? "",
        // Quoting a normal key does not change its identity. A literal dot inside
        // a quoted key, however, must never be confused with a table separator.
        StringValueSyntax { Value: { } value } => value.Contains('.') ? QuoteString(value) : value,
        _ => node?.ToString()?.Trim() ?? ""
    };

    private static string QuoteString(string value) {
        var sb = new System.Text.StringBuilder(value.Length + 2);
        sb.Append('"');
        foreach (var ch in value) {
            switch (ch) {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\b': sb.Append("\\b"); break;
                case '\t': sb.Append("\\t"); break;
                case '\n': sb.Append("\\n"); break;
                case '\f': sb.Append("\\f"); break;
                case '\r': sb.Append("\\r"); break;
                default:
                    if (char.IsControl(ch)) sb.Append($"\\u{(int)ch:X4}");
                    else sb.Append(ch);
                    break;
            }
        }
        sb.Append('"');
        return sb.ToString();
    }
}
