param(
    [string]$Path = (Join-Path $PSScriptRoot '../example-large.tif'),
    [int[]]$Sizes = @(128, 400),
    [int]$Iterations = 30
)
$ErrorActionPreference = 'Stop'
# Shell extraction benchmark only. This does not measure Explorer's UI or evict
# the filesystem cache. FORCEEXTRACTION ignores the thumbnail cache; INCACHEONLY
# measures it separately. No system cache is deleted.
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class ShellThumbnailBenchmark {
    [StructLayout(LayoutKind.Sequential)] public struct Size { public int Width, Height; }
    [ComImport, Guid("091162A4-BC96-411F-AAE8-C5122CD03363"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface ISharedBitmap {
        void GetSharedBitmap(out IntPtr bitmap);
        void GetSize(out Size size);
        void GetFormat(out uint format);
    }
    [ComImport, Guid("F676C15D-596A-4CE2-8234-33996F445DB1"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IThumbnailCache {
        void GetThumbnail(IntPtr item, uint size, uint flags,
            out ISharedBitmap bitmap, out uint cacheFlags, out Guid id);
    }
    [DllImport("shell32.dll", CharSet=CharSet.Unicode, PreserveSig=false)]
    static extern void SHCreateItemFromParsingName(string path, IntPtr context, ref Guid iid, out IntPtr item);
    public sealed class Sample {
        public double Milliseconds;
        public int Width, Height;
        public uint CacheFlags;
    }
    public static Sample[] Run(string path, uint size, uint flags, int count) {
        var iid = new Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE");
        IntPtr item;
        SHCreateItemFromParsingName(path, IntPtr.Zero, ref iid, out item);
        var cache = (IThumbnailCache)Activator.CreateInstance(Type.GetTypeFromCLSID(
            new Guid("50EF4544-AC9F-4A8E-B21B-8A26180DB13F")));
        try {
            var results = new Sample[count];
            for (int i=0; i<count; i++) {
                ISharedBitmap bitmap; uint cacheFlags; Guid id;
                var watch = Stopwatch.StartNew();
                cache.GetThumbnail(item, size, flags, out bitmap, out cacheFlags, out id);
                try {
                    Size dimensions; bitmap.GetSize(out dimensions);
                    IntPtr handle; bitmap.GetSharedBitmap(out handle);
                    watch.Stop();
                    results[i] = new Sample { Milliseconds=watch.Elapsed.TotalMilliseconds,
                        Width=dimensions.Width, Height=dimensions.Height, CacheFlags=cacheFlags };
                } finally { Marshal.ReleaseComObject(bitmap); }
            }
            return results;
        } finally { Marshal.ReleaseComObject(cache); Marshal.Release(item); }
    }
}
'@
$resolved = (Resolve-Path -LiteralPath $Path).Path
foreach ($size in $Sizes) {
    foreach ($mode in @(@{Name='forced_source';Flags=4}, @{Name='cache_only';Flags=1})) {
        $samples = [ShellThumbnailBenchmark]::Run($resolved, $size, $mode.Flags, $Iterations)
        $ordered = @($samples.Milliseconds | Sort-Object)
        [pscustomobject]@{
            path=$resolved; requested_size=$size; mode=$mode.Name
            filesystem_cache='uncontrolled; repeated reads generally warm'
            first_ms=$samples[0].Milliseconds
            median_ms=$ordered[[int][math]::Floor($ordered.Count / 2)]
            p95_ms=$ordered[[int][math]::Ceiling($ordered.Count * 0.95)-1]
            samples=$samples
        } | ConvertTo-Json -Depth 4 -Compress
    }
}
