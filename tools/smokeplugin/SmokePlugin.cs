using BepInEx;
using BepInEx.Unity.IL2CPP;

namespace clientpatch_smoke;

/// Smoke-test plugin for the local interop pipeline: proves BepInEx boots under
/// the game, loads our pre-generated interop assemblies, and can call into
/// UnityEngine through them. Check BepInEx/LogOutput.log after a game launch.
[BepInPlugin("dev.clientpatch.smoke", "clientpatch smoke", "1.0.0")]
public class SmokePlugin : BasePlugin
{
    public override void Load()
    {
        Log.LogInfo("[smoke] clientpatch smoke plugin loaded");
        try
        {
            Log.LogInfo($"[smoke] Application.unityVersion = {UnityEngine.Application.unityVersion}");
            Log.LogInfo($"[smoke] Application.version      = {UnityEngine.Application.version}");
            Log.LogInfo($"[smoke] Application.productName  = {UnityEngine.Application.productName}");
            Log.LogInfo("[smoke] interop calls OK — generated assemblies are binding");
        }
        catch (System.Exception e)
        {
            Log.LogError($"[smoke] interop call FAILED: {e}");
        }
    }
}
