function Write-UninstallFailureEvidence {
    param([string]$Install, [string]$Executable, [string]$Destination)

    $observation = [ordered]@{
        observed_at_utc = [DateTime]::UtcNow.ToString('o')
        executable_name = [IO.Path]::GetFileName($Executable)
        executable_exists = Test-Path -LiteralPath $Executable
        scope = 'Read-only failure evidence; no deletion, shutdown, restart or assertion retry.'
    }
    try {
        if (-not ('LoofahInstallerDiagnostics' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

public static class LoofahInstallerDiagnostics {
    [StructLayout(LayoutKind.Sequential)]
    public struct UniqueProcess { public uint Id; public FILETIME Started; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct ProcessInfo {
        public UniqueProcess Process;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string AppName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string ServiceName;
        public uint AppType;
        public uint Status;
        public uint Session;
        [MarshalAs(UnmanagedType.Bool)] public bool Restartable;
    }
    public class Owner {
        public uint Id;
        public long StartFileTime;
        public uint AppType;
        public uint Status;
        public uint Session;
    }
    public class Observation {
        public uint StartError;
        public uint RegisterError;
        public uint QueryError;
        public uint EndError;
        public uint RequiredCount;
        public uint RebootReasons;
        public bool Truncated;
        public Owner[] Owners = new Owner[0];
    }
    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    static extern uint RmStartSession(out uint session, uint flags, StringBuilder key);
    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    static extern uint RmRegisterResources(uint session, uint files, string[] names,
        uint applications, IntPtr processes, uint services, IntPtr serviceNames);
    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    static extern uint RmGetList(uint session, out uint needed, ref uint count,
        [In, Out] ProcessInfo[] processes, ref uint reasons);
    [DllImport("rstrtmgr.dll")]
    static extern uint RmEndSession(uint session);

    public static Observation Query(string file) {
        var result = new Observation();
        uint session;
        result.StartError = RmStartSession(out session, 0, new StringBuilder(33));
        if (result.StartError != 0) return result;
        try {
            result.RegisterError = RmRegisterResources(session, 1, new[] { file },
                0, IntPtr.Zero, 0, IntPtr.Zero);
            if (result.RegisterError != 0) return result;
            uint needed, count = 0, reasons = 0;
            result.QueryError = RmGetList(session, out needed, ref count, null, ref reasons);
            result.RequiredCount = needed;
            result.RebootReasons = reasons;
            if (result.QueryError == 0) return result;
            if (result.QueryError != 234) return result;
            if (needed > 64) { result.Truncated = true; return result; }
            var owners = new ProcessInfo[needed];
            count = needed;
            result.QueryError = RmGetList(session, out needed, ref count, owners, ref reasons);
            result.RequiredCount = needed;
            result.RebootReasons = reasons;
            if (result.QueryError != 0 || count > owners.Length) {
                result.Truncated = true;
                return result;
            }
            result.Owners = new Owner[count];
            for (int i = 0; i < count; i++) {
                var item = owners[i];
                result.Owners[i] = new Owner {
                    Id = item.Process.Id,
                    StartFileTime = ((long)item.Process.Started.dwHighDateTime << 32)
                        | (uint)item.Process.Started.dwLowDateTime,
                    AppType = item.AppType, Status = item.Status, Session = item.Session
                };
            }
            return result;
        } finally { result.EndError = RmEndSession(session); }
    }
}
'@
        }
        $observation.restart_manager = [LoofahInstallerDiagnostics]::Query($Executable)
    } catch { $observation.restart_manager_error = $_.Exception.GetType().FullName }
    try {
        $entries = @(Get-ChildItem -LiteralPath $Install -Force | Select-Object -First 65)
        $observation.entries_truncated = $entries.Count -gt 64
        $observation.entries = @($entries | Select-Object -First 64 | ForEach-Object {
            [ordered]@{ name = $_.Name; attributes = $_.Attributes.ToString(); length = $_.Length }
        })
    } catch { $observation.directory_error = $_.Exception.GetType().FullName }
    New-Item -ItemType Directory -Force (Split-Path $Destination -Parent) | Out-Null
    $observation | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Destination -Encoding utf8NoBOM
    Write-Output "Uninstall failure evidence saved to $Destination"
}
