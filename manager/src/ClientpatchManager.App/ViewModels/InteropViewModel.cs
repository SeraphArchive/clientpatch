using System.Collections.ObjectModel;
using System.Linq;
using System.Text;
using ClientpatchManager.App.I18n;
using ClientpatchManager.Core.Interop;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>Visual state of one interop generation step.</summary>
public enum StepState { Idle, Running, Done, Failed }

/// <summary>One of the three ordered interop steps shown on the page.</summary>
public sealed partial class InteropStepItem : ObservableObject
{
    public InteropStepItem(InteropStep step, string title, string detail)
    {
        Step = step;
        Title = title;
        Detail = detail;
    }

    public InteropStep Step { get; }
    public string Title { get; }
    public string Detail { get; }

    [ObservableProperty] private StepState _state = StepState.Idle;

    public bool IsRunning => State == StepState.Running;

    partial void OnStateChanged(StepState value) => OnPropertyChanged(nameof(IsRunning));
}

/// <summary>
/// Interop page VM: reports interop currency for the installed build, runs a Check, and
/// drives offline generation through <see cref="IInteropService.GenerateOfflineAsync"/>
/// with an <see cref="IProgress{T}"/> that advances the three-step display and appends to
/// the console. Offline-only: the runtime path is driven by clientpatch itself.
/// </summary>
public sealed partial class InteropViewModel : LocalizedViewModel
{
    private readonly IInteropService _interop;
    private readonly ClientpatchManager.Core.Config.IConfigService _config;
    private readonly Services.SettingsState _settings;
    private readonly StringBuilder _console = new();

    [ObservableProperty] private string _bannerText = "";
    [ObservableProperty] private bool _isCurrent;
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasConsole))]
    private string _consoleText = "";

    /// <summary>True once the console has output (hides the empty console card).</summary>
    public bool HasConsole => ConsoleText.Length > 0;
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(GenerateButtonText))]
    private bool _isGenerating;

    /// <summary>The Generate button reads "Generating" while the chain runs (mockup behavior).</summary>
    public string GenerateButtonText => IsGenerating ? Loc["interopGenRun"] : Loc["interopGen"];

    public ObservableCollection<InteropStepItem> Steps { get; } = new();

    public InteropViewModel(
        IInteropService interop,
        ClientpatchManager.Core.Config.IConfigService config,
        Services.SettingsState settings,
        LocalizationService loc) : base(loc)
    {
        _interop = interop;
        _config = config;
        _settings = settings;
        BuildSteps();
        Check();
    }

    /// <summary>The configured [interopdump] out_dir, when the config parses.</summary>
    private string? DumpRootName(string dir)
    {
        try
        {
            var configPath = ClientpatchManager.Core.Install.InstallLayout.Resolve(dir, "clientpatch.toml");
            if (!System.IO.File.Exists(configPath))
                return null;
            return _config.Load(System.IO.File.ReadAllText(configPath)).InteropDump?.OutDir;
        }
        catch
        {
            return null;
        }
    }

    private void BuildSteps()
    {
        Steps.Clear();
        Steps.Add(new InteropStepItem(InteropStep.Unpack, Loc["stepUnpack"], Loc["stepUnpackD"]));
        Steps.Add(new InteropStepItem(InteropStep.Dump, Loc["stepDump"], Loc["stepDumpD"]));
        Steps.Add(new InteropStepItem(InteropStep.Generate, Loc["stepGen"], Loc["stepGenD"]));
    }

    /// <summary>Check interop currency for the installed build.</summary>
    [RelayCommand]
    private void Check()
    {
        var dir = _settings.Current.GameDir;
        if (string.IsNullOrWhiteSpace(dir))
        {
            IsCurrent = false;
            BannerText = Loc["interopOutdated"];
            return;
        }

        try
        {
            var status = _interop.CheckCurrency(dir!, DumpRootName(dir!));
            IsCurrent = status.Current;
            BannerText = status.Current
                ? string.Format(Loc.Culture, Loc["interopCurrentFmt"], status.BuildId, status.AssemblyCount)
                : Loc["interopOutdated"];
        }
        catch
        {
            IsCurrent = false;
            BannerText = Loc["interopOutdated"];
        }
    }

    /// <summary>Run the offline generation chain, advancing the step display and console.</summary>
    [RelayCommand(CanExecute = nameof(CanGenerate))]
    private async Task GenerateAsync()
    {
        var dir = _settings.Current.GameDir;
        if (string.IsNullOrWhiteSpace(dir))
            return;

        IsGenerating = true;
        _console.Clear();
        ConsoleText = "";
        foreach (var s in Steps)
            s.State = StepState.Idle;

        // Mark the first step running immediately for responsive feedback.
        Steps[0].State = StepState.Running;

        var progress = new Progress<StepResult>(OnStep);
        // Live console: forward every tool line as it arrives (Progress<T> marshals to the UI thread).
        var console = new Progress<string>(Append);
        try
        {
            await _interop.GenerateOfflineAsync(dir!, DumpRootName(dir!), progress, CancellationToken.None, console);
            // Any step still idle/running that never reported stays idle; the last
            // reported result governs final state. Re-check currency afterwards.
            Check();
        }
        catch (Exception ex)
        {
            Append($"[error] {ex.Message}");
        }
        finally
        {
            // A step the chain stopped before (or a failed run that threw) would
            // otherwise keep its spinner forever.
            foreach (var s in Steps.Where(s => s.State == StepState.Running))
                s.State = StepState.Failed;
            IsGenerating = false;
        }
    }

    private bool CanGenerate() => !IsGenerating;

    /// <summary>Progress callback: update the reported step and start the next one.</summary>
    private void OnStep(StepResult result)
    {
        var index = Steps.ToList().FindIndex(s => s.Step == result.Step);
        if (index < 0)
            return;

        Steps[index].State = result.Ok ? StepState.Done : StepState.Failed;
        if (!string.IsNullOrWhiteSpace(result.Output))
            Append(result.Output);

        // Advance the next step to Running when this one succeeded.
        if (result.Ok && index + 1 < Steps.Count && Steps[index + 1].State == StepState.Idle)
            Steps[index + 1].State = StepState.Running;
    }

    private void Append(string line)
    {
        _console.AppendLine(line);
        ConsoleText = _console.ToString();
    }

    partial void OnIsGeneratingChanged(bool value) => GenerateCommand.NotifyCanExecuteChanged();

    protected override void OnLanguageChangedCore()
    {
        BuildSteps();
        OnPropertyChanged(nameof(GenerateButtonText));
        Check();
    }
}
