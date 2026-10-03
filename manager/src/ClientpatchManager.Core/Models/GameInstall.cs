namespace ClientpatchManager.Core.Models;

public enum DeploymentState { NotFound, GameOnly, Deployed }

public record GameInstall(
    string GameDir,
    string? VersionDllPath,
    string? ConfigPath,
    string? GameAssemblyPath,
    string? DeployedVersion,
    DeploymentState State);
