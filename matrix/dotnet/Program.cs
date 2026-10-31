// S3 SDK compatibility matrix for .NET: AWSSDK.S3 (aws-dotnet) and Minio (minio-dotnet). See ../SPEC.md.
using System.Diagnostics;
using System.Reflection;
using System.Text.Json;

namespace Matrix;

public sealed class AssertionFailed(string msg) : Exception(msg);

public sealed class SkipScenario(string reason) : Exception(reason);

public static class Env
{
    public static string Endpoint => Environment.GetEnvironmentVariable("S3_ENDPOINT") ?? throw new InvalidOperationException("S3_ENDPOINT required");
    public static string AccessKey => Environment.GetEnvironmentVariable("S3_ACCESS_KEY") ?? "fakes3";
    public static string SecretKey => Environment.GetEnvironmentVariable("S3_SECRET_KEY") ?? "fakes3secret";
    public static string Region => Environment.GetEnvironmentVariable("S3_REGION") ?? "us-east-1";
    public static string Server => Environment.GetEnvironmentVariable("MATRIX_SERVER") ?? throw new InvalidOperationException("MATRIX_SERVER required");
    public static string Out => Environment.GetEnvironmentVariable("MATRIX_OUT") ?? throw new InvalidOperationException("MATRIX_OUT required");
}

public static class Check
{
    public const int MiB = 1024 * 1024;

    public static void That(bool cond, string msg)
    {
        if (!cond) throw new AssertionFailed(msg);
    }

    static string Show(object? v)
    {
        var s = v switch
        {
            null => "null",
            string str => "\"" + str + "\"",
            System.Collections.IEnumerable e => "[" + string.Join(", ", e.Cast<object?>().Select(Show)) + "]",
            _ => v.ToString() ?? "",
        };
        return s.Length > 120 ? s[..120] : s;
    }

    public static void Eq<T>(T actual, T expected, string what)
    {
        That(EqualityComparer<T>.Default.Equals(actual, expected), $"{what}: expected {Show(expected)}, got {Show(actual)}");
    }

    public static void SeqEq<T>(IEnumerable<T>? actual, IEnumerable<T> expected, string what)
    {
        var a = (actual ?? []).ToList();
        var e = expected.ToList();
        That(a.SequenceEqual(e), $"{what}: expected {Show(e)}, got {Show(a)}");
    }

    public static string Unquote(string? etag) => (etag ?? "").Replace("\"", "");

    public static string Md5Hex(byte[] data) => Convert.ToHexStringLower(System.Security.Cryptography.MD5.HashData(data));

    public static byte[] Random(int n)
    {
        var b = new byte[n];
        System.Random.Shared.NextBytes(b);
        return b;
    }

    public static string RandHex(int bytes) => Convert.ToHexStringLower(Random(bytes));

    public static readonly byte[] RangeData = Enumerable.Range(0, 100).Select(i => (byte)i).ToArray();
}

/// Independent checksum implementations for verifying what the server returns.
public static class Checksums
{
    static readonly uint[] Crc32Table = Table32(0xEDB88320);
    static readonly uint[] Crc32CTable = Table32(0x82F63B78);
    static readonly ulong[] Crc64Table = Table64(0x9A6C9329AC4BC9B5);

    static uint[] Table32(uint poly) => Enumerable.Range(0, 256).Select(i =>
    {
        var c = (uint)i;
        for (var k = 0; k < 8; k++) c = (c & 1) != 0 ? (c >> 1) ^ poly : c >> 1;
        return c;
    }).ToArray();

    static ulong[] Table64(ulong poly) => Enumerable.Range(0, 256).Select(i =>
    {
        var c = (ulong)i;
        for (var k = 0; k < 8; k++) c = (c & 1) != 0 ? (c >> 1) ^ poly : c >> 1;
        return c;
    }).ToArray();

    static byte[] Crc32(byte[] data, uint[] table)
    {
        var c = 0xFFFFFFFFu;
        foreach (var b in data) c = table[(c ^ b) & 0xff] ^ (c >> 8);
        c ^= 0xFFFFFFFFu;
        return [(byte)(c >> 24), (byte)(c >> 16), (byte)(c >> 8), (byte)c];
    }

    static byte[] Crc64(byte[] data)
    {
        var c = ulong.MaxValue;
        foreach (var b in data) c = Crc64Table[(c ^ b) & 0xff] ^ (c >> 8);
        c ^= ulong.MaxValue;
        var r = new byte[8];
        System.Buffers.Binary.BinaryPrimitives.WriteUInt64BigEndian(r, c);
        return r;
    }

    public static string Expected(string algo, byte[] data) => Convert.ToBase64String(algo switch
    {
        "CRC32" => Crc32(data, Crc32Table),
        "CRC32C" => Crc32(data, Crc32CTable),
        "CRC64NVME" => Crc64(data),
        "SHA1" => System.Security.Cryptography.SHA1.HashData(data),
        "SHA256" => System.Security.Cryptography.SHA256.HashData(data),
        _ => throw new ArgumentException(algo),
    });
}

public delegate Task ScenarioFn(CancellationToken ct);

public static class Runner
{
    const int TimeoutSeconds = 60;

    public static async Task Run(string sdk, string version, IEnumerable<(string Id, ScenarioFn Fn)> scenarios,
        Func<Exception, string> describe, Func<Task> cleanup)
    {
        var results = new List<Dictionary<string, object>>();
        foreach (var (id, fn) in scenarios)
        {
            var res = new Dictionary<string, object> { ["scenario"] = id };
            var sw = Stopwatch.StartNew();
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(TimeoutSeconds));
            try
            {
                await fn(cts.Token).WaitAsync(TimeSpan.FromSeconds(TimeoutSeconds));
                res["status"] = "pass";
            }
            catch (SkipScenario e)
            {
                res["status"] = "skip";
                res["error"] = e.Message;
            }
            catch (Exception e)
            {
                res["status"] = "fail";
                res["error"] = e is TimeoutException || (e is OperationCanceledException && cts.IsCancellationRequested)
                    ? $"Timeout: scenario exceeded {TimeoutSeconds}s"
                    : Truncate(describe(Unwrap(e)));
            }
            if ((string)res["status"] != "skip") res["ms"] = Math.Round(sw.Elapsed.TotalMilliseconds, 1);
            Console.Error.WriteLine($"[{sdk}] {id,-30} {res["status"],-4} {(res.TryGetValue("error", out var err) ? err : "")}");
            results.Add(res);
        }

        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(120));
            await cleanup().WaitAsync(cts.Token);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[{sdk}] cleanup failed: {e.Message}");
        }

        var doc = new Dictionary<string, object>
        {
            ["sdk"] = sdk,
            ["lang"] = "dotnet",
            ["version"] = version,
            ["server"] = Env.Server,
            ["results"] = results,
        };
        var json = JsonSerializer.Serialize(doc, new JsonSerializerOptions
        {
            WriteIndented = true,
            Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
        });
        await File.WriteAllTextAsync(Path.Combine(Env.Out, sdk + ".json"), json + "\n");
    }

    static Exception Unwrap(Exception e)
    {
        while (e is AggregateException { InnerException: not null } || e is TargetInvocationException { InnerException: not null })
            e = e.InnerException!;
        return e;
    }

    public static string FirstLine(string? s) => (s ?? "").Split('\n')[0].Trim();

    static string Truncate(string s) => s.Length > 300 ? s[..300] : s;

    public static string PackageVersion(Assembly asm)
    {
        var v = asm.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
                ?? asm.GetCustomAttribute<AssemblyFileVersionAttribute>()?.Version
                ?? asm.GetName().Version?.ToString() ?? "unknown";
        var plus = v.IndexOf('+');
        return plus >= 0 ? v[..plus] : v;
    }
}

public static class Program
{
    public static async Task<int> Main()
    {
        await new AwsMatrix().RunAll();
        await new MinioMatrix().RunAll();
        return 0;
    }
}
