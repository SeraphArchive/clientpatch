using System.Xml.Linq;
using Xunit;

public class ResourceParityTests {
    static readonly string[] Cultures = { "", "zh-Hans", "zh-Hant", "ja", "ko" };

    static string ResxPath(string c) => Path.Combine(
        AppContext.BaseDirectory, "Resources",
        c == "" ? "Strings.resx" : $"Strings.{c}.resx");

    static HashSet<string> Keys(string path) => XDocument.Load(path)
        .Root!.Elements("data").Select(e => e.Attribute("name")!.Value).ToHashSet();

    [Fact] public void All_cultures_have_the_same_keys() {
        var baseKeys = Keys(ResxPath(""));
        foreach (var c in Cultures.Where(c => c != "")) {
            var k = Keys(ResxPath(c));
            Assert.True(baseKeys.SetEquals(k), $"culture {c} key mismatch: missing {string.Join(",", baseKeys.Except(k))}; extra {string.Join(",", k.Except(baseKeys))}");
        }
    }
}
