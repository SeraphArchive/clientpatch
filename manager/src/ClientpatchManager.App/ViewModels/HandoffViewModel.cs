using System.Collections.ObjectModel;
using System.Linq;
using System.IO;
using System.Text;
using ClientpatchManager.App.I18n;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Launch;
using CommunityToolkit.Mvvm.ComponentModel;

namespace ClientpatchManager.App.ViewModels;

/// <summary>
/// Drives one clientpatch handoff. The game has already exited. This window is a separate
/// instance from the main shell: it finishes the unmet startup condition (generate interop,
/// install the BepInEx payload, or just relaunch), shows that progress here, restarts the
/// game, and then this process exits.
/// </summary>
public sealed partial class HandoffViewModel : LocalizedViewModel
{
    /// <summary>Marker file (in the dump dir) written on a successful generation; content = the UTC completion timestamp (round-trip "O" format).</summary>
    public const string DoneMarker = "manager.done";

    /// <summary>Marker file (in the dump dir) written on failure; content = the error text.</summary>
    public const string FailedMarker = "manager.failed";

    private readonly IInteropService _interop;
    private readonly IPayloadInstaller _payload;
    private readonly ILauncher _launcher;
    private readonly IInstallDetector _detector;
    private readonly StringBuilder _console = new();
    private readonly SetupRequest _request;

    public ObservableCollection<InteropStepItem> Steps { get; } = new();

    /// <summary>Raised once the handoff finishes. True means the job succeeded and a requested
    /// relaunch was started, so this instance can exit. False leaves the window open.</summary>
    public event Action<bool>? Completed;

    public HandoffViewModel(
        IInteropService interop,
        IPayloadInstaller payload,
        ILauncher launcher,
        IInstallDetector detector,
        LocalizationService loc,
        SetupRequest request) : base(loc)
    {
        _interop = interop;
        _payload = payload;
        _launcher = launcher;
        _detector = detector;
        _request = request;
        BuildSteps();
    }

    private void BuildSteps()
    {
        Steps.Clear();
        switch (_request.Kind)
        {
            case SetupKind.Generate:
                // The runtime-dump chain has no senbei Unpack step.
                Steps.Add(new InteropStepItem(InteropStep.Dump, Loc["stepDump"], Loc["stepDumpD"]));
                Steps.Add(new InteropStepItem(InteropStep.Generate, Loc["stepGen"], Loc["stepGenD"]));
                break;
            case SetupKind.Payload:
                Steps.Add(new InteropStepItem(InteropStep.Install, Loc["stepInstall"], Loc["stepInstallD"]));
                break;
        }
        if (_request.Relaunch)
            Steps.Add(new InteropStepItem(InteropStep.Relaunch, Loc["stepRelaunch"], Loc["stepRelaunchD"]));
    }

    public string Title => Loc["handoffTitle"];

    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _consoleText = "";
    [ObservableProperty] private bool _isRunning;

    /// <summary>
    /// Runs the handoff. Never throws: any failure is reported through the console and a false
    /// <see cref="Completed"/> signal, and the window stays open.
    /// </summary>
    public async Task RunAsync()
    {
        try
        {
            await RunCoreAsync();
        }
        catch (Exception ex)
        {
            // Last-resort guard: the handoff must never die with the game already closed.
            Append($"[error] {ex.Message}");
            StatusText = Loc["handoffFailed"];
            IsRunning = false;
            try { WriteMarker(false, ex.Message); } catch { /* marker is best-effort here */ }
            Completed?.Invoke(false);
        }
    }

    private async Task RunCoreAsync()
    {
        if (_request.ParentPid is { } pid) {
            try {
                using var parent = System.Diagnostics.Process.GetProcessById(pid);
                using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(15));
                await parent.WaitForExitAsync(timeout.Token);
            }
            catch (ArgumentException) { /* already exited */ }
            if (!string.IsNullOrWhiteSpace(_request.GameDir)) {
                using var shutdownTimeout = new CancellationTokenSource(TimeSpan.FromSeconds(15));
                await ClientpatchManager.Core.Install.GameOperation.WaitForStoppedAsync(_request.GameDir!, shutdownTimeout.Token);
            }
        }
        IsRunning = true;
        StatusText = _request.Kind switch
        {
            SetupKind.Payload => Loc["handoffPayload"],
            SetupKind.Relaunch => Loc["handoffRelaunching"],
            _ => Loc["handoffRunning"],
        };
        if (Steps.Count > 0)
            Steps[0].State = StepState.Running;

        var jobOk = false;
        try
        {
            jobOk = _request.Kind switch
            {
                SetupKind.Generate => await GenerateAsync(),
                SetupKind.Payload => await InstallPayloadAsync(),
                SetupKind.Relaunch => true,
                _ => false,
            };
        }
        catch (Exception ex)
        {
            Append($"[error] {ex.Message}");
        }

        try
        {
            WriteMarker(jobOk, jobOk ? null : LastError());
        }
        catch (Exception ex)
        {
            Append($"[warn] could not write marker: {ex.Message}");
        }

        var relaunchOk = true;
        if (jobOk && _request.Relaunch)
            relaunchOk = Relaunch();

        if (jobOk && relaunchOk)
        {
            // A step that never failed (a Dump row the dumper skipped, the relaunch row) is
            // settled to Done so a successful run never shows a perpetual spinner.
            foreach (var s in Steps.Where(s => s.State != StepState.Failed))
                s.State = StepState.Done;
            if (!_request.Relaunch)
                StatusText = Loc["handoffDone"];
        }
        else
        {
            foreach (var s in Steps.Where(s => s.State == StepState.Running))
                s.State = StepState.Failed;
            if (jobOk)
                StatusText = Loc["handoffRelaunchFailed"];
            else if (_request.Kind == SetupKind.Payload)
                StatusText = Loc["handoffPayloadFailed"];
            else
                StatusText = Loc["handoffFailed"];
        }

        IsRunning = false;
        Completed?.Invoke(jobOk && relaunchOk);
    }

    private async Task<bool> GenerateAsync()
    {
        if (string.IsNullOrWhiteSpace(_request.DumpDir))
        {
            Append($"[error] {Loc["handoffFailed"]}");
            return false;
        }
        var progress = new Progress<StepResult>(OnStep);
        var console = new Progress<string>(Append);
        var results = await _interop.GenerateFromDumpAsync(
            _request.DumpDir!, _request.GameDir, progress, CancellationToken.None, console);
        var ok = results.Any(r => r.Step == InteropStep.Generate && r.Ok);
        if (!ok)
            Append($"[error] {Loc["handoffFailed"]}");
        return ok;
    }

    private async Task<bool> InstallPayloadAsync()
    {
        if (string.IsNullOrWhiteSpace(_request.GameDir))
        {
            Append($"[error] {Loc["handoffNoGameDir"]}");
            return false;
        }
        if (string.IsNullOrWhiteSpace(_request.PayloadUrl))
        {
            Append("[error] bepinex url is empty");
            return false;
        }
        Append(_request.PayloadUrl!);
        await _payload.InstallAsync(
            _request.GameDir!, _request.PayloadUrl!, _request.DoorstopName, _request.BepInExRoot, CancellationToken.None);
        var step = Steps.FirstOrDefault(s => s.Step == InteropStep.Install);
        if (step is not null)
            step.State = StepState.Done;
        var next = Steps.FirstOrDefault(s => s.Step == InteropStep.Relaunch);
        if (next is { State: StepState.Idle })
            next.State = StepState.Running;
        return true;
    }

    /// <summary>Progress callback: mark the reported step done/failed and start the next one.</summary>
    private void OnStep(StepResult result)
    {
        var index = Steps.ToList().FindIndex(s => s.Step == result.Step);
        if (index < 0)
            return;

        Steps[index].State = result.Ok ? StepState.Done : StepState.Failed;
        if (!string.IsNullOrWhiteSpace(result.Output))
            Append(result.Output);

        if (result.Ok && index + 1 < Steps.Count && Steps[index + 1].State == StepState.Idle)
            Steps[index + 1].State = StepState.Running;
    }

    /// <summary>Starts the game. Returns false when it could not, leaving the reason in the console.
    /// The window then stays open: the game is already closed, so a silent failure would leave it that way.</summary>
    private bool Relaunch()
    {
        if (string.IsNullOrWhiteSpace(_request.GameDir))
        {
            StatusText = Loc["handoffNoGameDir"];
            Append($"[warn] {Loc["handoffNoGameDir"]}");
            return false;
        }

        try
        {
            StatusText = Loc["handoffRelaunching"];
            var install = _detector.Detect(_request.GameDir!);
            _launcher.Launch(install, _request.LaunchMethod);
            Append($"[info] relaunched via {_request.LaunchMethod}");
            return true;
        }
        catch (Exception ex)
        {
            Append($"[warn] relaunch failed: {ex.Message}");
            return false;
        }
    }

    private void WriteMarker(bool success, string? error)
    {
        // Only the generate job has a dump dir to stamp. Payload and relaunch have nothing to mark.
        if (string.IsNullOrWhiteSpace(_request.DumpDir))
            return;
        Directory.CreateDirectory(_request.DumpDir!);
        var donePath = Path.Combine(_request.DumpDir!, DoneMarker);
        var failPath = Path.Combine(_request.DumpDir!, FailedMarker);
        if (success)
        {
            if (File.Exists(failPath)) File.Delete(failPath);
            File.WriteAllText(donePath, DateTimeOffset.UtcNow.ToString("O"));
        }
        else
        {
            if (File.Exists(donePath)) File.Delete(donePath);
            File.WriteAllText(failPath, error ?? "generation failed");
        }
    }

    private string LastError()
    {
        // Best-effort: the console's tail carries the failing step's captured output.
        var text = _console.ToString().TrimEnd();
        return text.Length == 0 ? "generation failed" : text;
    }

    private void Append(string line)
    {
        _console.AppendLine(line);
        ConsoleText = _console.ToString();
    }
}
