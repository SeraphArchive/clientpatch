namespace ClientpatchManager.App.Services;

/// <summary>
/// Process-wide result of the most recent update check, written by UpdatesViewModel and
/// read by the Home page so a launch-time check is visible without opening Updates.
/// </summary>
public sealed class UpdateAvailability
{
    public string? LatestVersion { get; private set; }
    public bool Available { get; private set; }

    public event Action? Changed;

    public void Set(string? latestVersion, bool available)
    {
        if (latestVersion == LatestVersion && available == Available)
            return;
        LatestVersion = latestVersion;
        Available = available;
        Changed?.Invoke();
    }
}
