using Tomlyn;
using Tomlyn.Model;

namespace ClientpatchManager.Core.Config;

/// <summary>Mirrors the Rust proxy's schema, including dotted and inline TOML tables.</summary>
internal static class ConfigValidation {
    internal static TomlTable Parse(string text) {
        var root = TomlSerializer.Deserialize<TomlTable>(text) ?? new TomlTable();
        NormalizeEnable(root);
        foreach (var section in new[] { "loader", "lilypad", "steam", "isolation", "titlebar", "interopdump", "bepinex", "lilypad.report" }) {
            var table = GetTable(root, section);
            if (table is null) continue;
            var strings = section switch {
                "loader" => "log", "lilypad" => "api_base platform_base signing server_public_key_pem",
                "steam" => "mode ip_country ui_language", "isolation" => "suffix", "titlebar" => "template",
                "interopdump" => "out_dir", "lilypad.report" => "url", "bepinex" => "root doorstop url interopgen manager launch_method", _ => ""
            };
            var bools = section switch {
                "loader" => "crash_test", "lilypad" => "neutralize_debugger_flag capture_md5", "interopdump" => "force restore_handles",
                "bepinex" => "auto_download auto_generate", _ => ""
            };
            if (section != "loader") bools += " enable";
            foreach (var key in strings.Split(' ', StringSplitOptions.RemoveEmptyEntries))
                if (table.TryGetValue(key, out var field) && field is not string) throw new FormatException($"{section}.{key} must be a string");
            foreach (var key in bools.Split(' ', StringSplitOptions.RemoveEmptyEntries))
                if (table.TryGetValue(key, out var field) && field is not bool) throw new FormatException($"{section}.{key} must be a boolean");
            string S(string key, string fallback) => table.TryGetValue(key, out var item) ? (string)item : fallback;
            void NonEmpty(string key, string fallback) { if (string.IsNullOrWhiteSpace(S(key, fallback))) throw new FormatException($"{section}.{key} is empty"); }
            var active = table.TryGetValue("enable", out var enabled) && enabled is true;
            switch (section) {
                case "lilypad":
                    if (!active) break;
                    NonEmpty("api_base", ""); NonEmpty("platform_base", "");
                    var signing = S("signing", "noop");
                    if (signing is not ("noop" or "rsa")) throw new FormatException("lilypad.signing must be noop or rsa");
                    if (signing == "rsa") NonEmpty("server_public_key_pem", "");
                    break;
                case "steam":
                    if (!active) break;
                    if (S("mode", "auto").Trim().ToLowerInvariant() is not ("off" or "false" or "none" or "skip_restart" or "skip-restart" or "skip" or "stub" or "emu" or "offline" or "auto")) throw new FormatException("steam.mode is unsupported");
                    NonEmpty("ip_country", "JP"); NonEmpty("ui_language", "japanese"); break;
                case "isolation":
                    if (!active) break;
                    var suffix = S("suffix", "clientpatch");
                    if (suffix.Length is < 1 or > 64 || suffix.StartsWith('.') || suffix.EndsWith('.') || suffix.Contains("..")
                        || suffix.Any(c => !char.IsAsciiLetterOrDigit(c) && c is not ('_' or '-' or '.')))
                        throw new FormatException("isolation.suffix must be 1-64 ASCII letters, digits, _, - or interior single dots");
                    break;
                case "interopdump": if (active) NonEmpty("out_dir", "interop"); break;
                case "bepinex":
                    NonEmpty("root", "BepInEx"); NonEmpty("doorstop", "doorstop.dll"); NonEmpty("interopgen", "clientpatch\\interopgen.ps1");
                    foreach (var key in new[] { "root", "doorstop" }) {
                        var path = S(key, key == "root" ? "BepInEx" : "doorstop.dll");
                        if (Path.IsPathRooted(path) || path.Contains(':') || path.Replace('\\', '/').Split('/').Contains("..")) throw new FormatException($"bepinex.{key} must stay inside the game directory");
                    }
                    if (S("doorstop", "doorstop.dll").Equals("winhttp.dll", StringComparison.OrdinalIgnoreCase)) throw new FormatException("bepinex.doorstop must not autoload as winhttp.dll");
                    var method = S("launch_method", "steam").Trim().ToLowerInvariant();
                    if (method is not ("" or "steam" or "direct")) throw new FormatException("bepinex.launch_method must be steam or direct");
                    if (table.TryGetValue("max_restarts", out var restarts) && (restarts is not long n || n < 0 || n > uint.MaxValue)) throw new FormatException("bepinex.max_restarts must be an unsigned 32-bit integer");
                    if (active && (!table.TryGetValue("auto_download", out var download) || download is true)) NonEmpty("url", "default");
                    break;
            }
        }
        return root;
    }

    static void NormalizeEnable(TomlTable root) {
        if (root.Remove("regredirect", out var legacyIsolation) && !root.ContainsKey("isolation"))
            root["isolation"] = legacyIsolation;
        if (root.TryGetValue("report", out var legacyReport)) {
            root.Remove("report");
            if (!root.ContainsKey("lilypad")) root["lilypad"] = new TomlTable { ["enable"] = false };
            if (root["lilypad"] is TomlTable parent && !parent.ContainsKey("report")) parent["report"] = legacyReport;
        }
        List<string>? legacy = null;
        if (root.TryGetValue("loader", out var loaderValue) && loaderValue is TomlTable loader && loader.TryGetValue("modules", out var items)) {
            if (items is not TomlArray array || array.Any(item => item is not string))
                throw new FormatException("loader.modules must be an array of strings");
            legacy = array.Cast<string>().Select(name => name == "regredirect" ? "isolation" : name).ToList(); loader.Remove("modules");
        }
        foreach (var name in new[] { "lilypad", "steam", "isolation", "titlebar", "interopdump", "bepinex" }) {
            if (!root.ContainsKey(name) && name is not ("lilypad" or "bepinex") && legacy?.Contains(name) == true)
                root[name] = new TomlTable();
            if (!root.TryGetValue(name, out var value) || value is not TomlTable table) continue;
            table.TryGetValue("enabled", out var old); table.Remove("enabled");
            if (table.ContainsKey("enable")) continue;
            if (legacy is not null) {
                if (name == "bepinex" && old is not null && old is not bool)
                    throw new FormatException("bepinex.enabled must be a boolean");
                table["enable"] = legacy.Contains(name) && (name != "bepinex" || old is true);
            } else table["enable"] = old ?? false;
        }
        if (GetTable(root, "lilypad.report") is { } report) {
            report.TryGetValue("enabled", out var old); report.Remove("enabled");
            if (!report.ContainsKey("enable")) report["enable"] = old ?? true;
        }
    }

    internal static TomlTable? GetTable(TomlTable root, string path) {
        var current = root;
        foreach (var key in path.Split('.')) {
            if (!current.TryGetValue(key, out var value)) return null;
            if (value is not TomlTable table) throw new FormatException($"{path} must be a table");
            current = table;
        }
        return current;
    }
}
