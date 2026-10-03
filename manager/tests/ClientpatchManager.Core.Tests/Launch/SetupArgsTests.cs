using ClientpatchManager.Core.Launch;

public class SetupArgsTests {
    [Fact]
    public void Normal_launch_is_not_a_handoff() {
        Assert.Null(SetupArgs.Parse(new[] { "--something" }));
        Assert.Null(SetupArgs.Parse(Array.Empty<string>()));
    }

    [Fact]
    public void Generate_handoff_keeps_the_dump_and_the_game() {
        var req = SetupArgs.Parse(new[] {
            "--generate-from-dump", @"D:\game\interop\abc",
            "--relaunch", "--launch-method", "direct", "--game-dir", @"D:\game",
        });
        Assert.NotNull(req);
        Assert.Equal(SetupKind.Generate, req!.Kind);
        Assert.Equal(@"D:\game\interop\abc", req.DumpDir);
        Assert.Equal(@"D:\game", req.GameDir);
        Assert.True(req.Relaunch);
        Assert.Equal(LaunchMethod.Direct, req.LaunchMethod);
    }

    [Fact]
    public void Payload_handoff_carries_the_zip_and_the_doorstop_name() {
        var req = SetupArgs.Parse(new[] {
            "--setup", "payload",
            "--payload-url", "https://example/bepinex.zip",
            "--doorstop", "doorstop.dll",
            "--bepinex-root", "BepInEx",
            "--relaunch", "--launch-method", "steam", "--game-dir", @"D:\game",
        });
        Assert.NotNull(req);
        Assert.Equal(SetupKind.Payload, req!.Kind);
        Assert.Equal("https://example/bepinex.zip", req.PayloadUrl);
        Assert.Equal("doorstop.dll", req.DoorstopName);
        Assert.Equal("BepInEx", req.BepInExRoot);
        Assert.Equal(LaunchMethod.Steam, req.LaunchMethod);
    }

    [Fact]
    public void Relaunch_handoff_does_not_require_a_dump() {
        var req = SetupArgs.Parse(new[] {
            "--setup", "relaunch", "--relaunch", "--game-dir", @"D:\game",
        });
        Assert.NotNull(req);
        Assert.Equal(SetupKind.Relaunch, req!.Kind);
        Assert.Null(req.DumpDir);
        Assert.True(req.Relaunch);
    }

    [Fact]
    public void Missing_launch_method_defaults_to_steam() {
        var req = SetupArgs.Parse(new[] { "--generate-from-dump", "dump" });
        Assert.Equal(LaunchMethod.Steam, req!.LaunchMethod);
        Assert.False(req.Relaunch);
    }
}
