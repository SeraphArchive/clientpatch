namespace ClientpatchManager.Core.Update;

/// <summary>
/// Real <see cref="IFileDownloader"/>: streams a URL to a temp file via HttpClient.
/// Not exercised by unit tests (which inject a fake); kept minimal and network-free at rest.
/// </summary>
public sealed class HttpFileDownloader : IFileDownloader {
    readonly HttpClient _http;

    public HttpFileDownloader(HttpClient http) {
        _http = http;
    }

    public async Task<string> DownloadToTempAsync(string url, CancellationToken ct) {
        var temp = Path.GetTempFileName();
        try {
            using var response = await _http.GetAsync(url, HttpCompletionOption.ResponseHeadersRead, ct).ConfigureAwait(false);
            response.EnsureSuccessStatusCode();
            await using var src = await response.Content.ReadAsStreamAsync(ct).ConfigureAwait(false);
            await using var dst = File.Create(temp);
            await src.CopyToAsync(dst, ct).ConfigureAwait(false);
            return temp;
        }
        catch {
            // Never orphan the temp file on a failed download.
            try { if (File.Exists(temp)) File.Delete(temp); }
            catch { /* best-effort cleanup */ }
            throw;
        }
    }
}
