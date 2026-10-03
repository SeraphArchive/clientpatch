using System.Globalization;
using System.Resources;

namespace ClientpatchManager.App.I18n;

/// <summary>
/// Wraps a <see cref="ResourceManager"/> over the Strings.*.resx resources and a
/// current <see cref="CultureInfo"/>. Switching languages raises
/// <see cref="LanguageChanged"/> so views can re-read their bound strings.
/// </summary>
public sealed class LocalizationService
{
    private readonly ResourceManager _rm;
    private CultureInfo _culture;

    public LocalizationService()
    {
        _rm = new ResourceManager("ClientpatchManager.App.Resources.Strings", typeof(LocalizationService).Assembly);
        _culture = CultureInfo.CurrentUICulture;
    }

    /// <summary>Raised after the active language changes so views re-read strings.</summary>
    public event Action? LanguageChanged;

    /// <summary>The currently active culture.</summary>
    public CultureInfo Culture => _culture;

    /// <summary>
    /// Switch the active language. An empty culture follows the system UI culture (the
    /// resource manager's parent chain maps e.g. zh-CN to the zh-Hans satellite); an
    /// invalid culture falls back to the invariant culture (English). Raises
    /// <see cref="LanguageChanged"/>.
    /// </summary>
    public void SetLanguage(string culture)
    {
        if (string.IsNullOrWhiteSpace(culture))
        {
            _culture = CultureInfo.CurrentUICulture;
        }
        else
        {
            try
            {
                _culture = CultureInfo.GetCultureInfo(culture);
            }
            catch (CultureNotFoundException)
            {
                _culture = CultureInfo.InvariantCulture;
            }
        }
        LanguageChanged?.Invoke();
    }

    /// <summary>Look up a string by key for the active culture; returns the key if missing.</summary>
    public string this[string key] => _rm.GetString(key, _culture) ?? key;
}
