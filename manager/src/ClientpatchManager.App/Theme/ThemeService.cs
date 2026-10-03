using System.Windows;
using System.Windows.Interop;
using Wpf.Ui.Appearance;
using Wpf.Ui.Controls;

namespace ClientpatchManager.App.Theme;

/// <summary>
/// Applies the Wpf.Ui application theme from a settings string. "system" follows
/// the OS appearance (and keeps following it while the app runs via
/// <see cref="SystemThemeWatcher"/>); "light"/"dark" pin the theme. Applying is
/// two-phase: once before the window is shown (theme dictionaries) and again from
/// <c>SourceInitialized</c> (the Mica backdrop and the watcher need an HWND). A
/// single post-Show apply leaves the window chrome black until the next toggle.
/// </summary>
public sealed class ThemeService
{
    /// <summary>The backdrop used for every window. None: a flat theme surface. Real Mica
    /// samples the desktop and renders the title strip near-black on dark wallpapers; Mica
    /// Alt (Tabbed) tints even harder. The mockup's surface is a soft static gradient, so a
    /// deterministic flat surface is both closer to the design and identical on every machine.</summary>
    public const WindowBackdropType Backdrop = WindowBackdropType.None;

    private Window? _watchedWindow;
    private bool _watching;

    /// <summary>
    /// Apply a theme by settings token: "light", "dark", or "system" (default).
    /// Unknown values are treated as "system".
    /// </summary>
    public void Apply(string? theme)
    {
        switch ((theme ?? "system").Trim().ToLowerInvariant())
        {
            case "light":
                StopWatching();
                ApplicationThemeManager.Apply(ApplicationTheme.Light, Backdrop, updateAccent: true);
                break;

            case "dark":
                StopWatching();
                ApplicationThemeManager.Apply(ApplicationTheme.Dark, Backdrop, updateAccent: true);
                break;

            default: // "system"
                var system = ApplicationThemeManager.GetSystemTheme();
                var resolved = system == SystemTheme.Dark ? ApplicationTheme.Dark : ApplicationTheme.Light;
                ApplicationThemeManager.Apply(resolved, Backdrop, updateAccent: true);
                if (_watchedWindow is not null && HasHandle(_watchedWindow) && !_watching)
                {
                    SystemThemeWatcher.Watch(_watchedWindow, Backdrop, updateAccents: true);
                    _watching = true;
                }
                break;
        }
    }

    /// <summary>
    /// Re-apply only the window backdrop. Insurance against a first-composition glitch
    /// that leaves the chrome un-blended until something forces a redraw (the manual
    /// fix is toggling the theme); called from <c>ContentRendered</c>. Does not touch
    /// the theme dictionaries or the watcher, so a healthy start pays nothing.
    /// </summary>
    public void ReapplyBackdrop()
    {
        if (_watchedWindow is not null && HasHandle(_watchedWindow))
            WindowBackdrop.ApplyBackdrop(_watchedWindow, Backdrop);
    }

    /// <summary>
    /// Register the main window so "system" mode can track live OS theme changes.
    /// Call once after the window is created, before <see cref="Apply"/>.
    /// </summary>
    public void SetWindow(Window window) => _watchedWindow = window;

    private void StopWatching()
    {
        // WPF-UI throws when asked to unwatch a window that has no handle yet.
        if (_watching && _watchedWindow is not null && HasHandle(_watchedWindow))
            SystemThemeWatcher.UnWatch(_watchedWindow);
        _watching = false;
    }

    private static bool HasHandle(Window window) =>
        new WindowInteropHelper(window).Handle != IntPtr.Zero;
}
