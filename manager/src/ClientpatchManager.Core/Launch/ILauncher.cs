using ClientpatchManager.Core.Models;

namespace ClientpatchManager.Core.Launch;

public enum LaunchMethod { Steam, Direct }

public interface ILauncher {
    void Launch(GameInstall install, LaunchMethod method);
}
