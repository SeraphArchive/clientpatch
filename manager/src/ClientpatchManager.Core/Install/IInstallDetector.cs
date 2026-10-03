using ClientpatchManager.Core.Models;

namespace ClientpatchManager.Core.Install;

public interface IInstallDetector {
    GameInstall Detect(string gameDir);
}
