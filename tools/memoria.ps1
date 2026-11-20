param([int]$ProcessId, [int]$Top = 20)
# Windows: reparte el conjunto de trabajo privado de un proceso (lo que el Administrador de tareas
# llama "Memoria") por regiones: datos privados de cada DLL, archivos mapeados y reservas privadas
# (montón de Garbo, memoria del controlador de la GPU…), con VirtualQueryEx y QueryWorkingSetEx.
#   powershell -File tools\memoria.ps1 -ProcessId <pid de garbo>
if (-not ("GarboMem" -as [type])) {
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class GarboMem {
    [StructLayout(LayoutKind.Sequential)] struct MBI {
        public IntPtr BaseAddress, AllocationBase; public uint AllocationProtect; public ushort PartitionId;
        public IntPtr RegionSize; public uint State, Protect, Type;
    }
    [StructLayout(LayoutKind.Sequential)] struct WSEX { public IntPtr Va; public IntPtr Attr; }
    [DllImport("kernel32")] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32")] static extern IntPtr VirtualQueryEx(IntPtr h, IntPtr addr, out MBI m, IntPtr len);
    [DllImport("psapi", CharSet = CharSet.Unicode)] static extern uint GetMappedFileNameW(IntPtr h, IntPtr addr, StringBuilder sb, uint n);
    [DllImport("psapi")] static extern bool QueryWorkingSetEx(IntPtr h, [In, Out] WSEX[] pv, uint cb);

    public static List<string> Analyze(int pid, int top) {
        IntPtr h = OpenProcess(0x0400 | 0x0010, false, pid); // QUERY_INFORMATION | VM_READ
        var byKey = new Dictionary<string, long>();
        var byAlloc = new Dictionary<long, long>();
        var allocSize = new Dictionary<long, long>();
        long addr = 0, total = 0, soloAqui = 0;
        MBI m;
        int size = Marshal.SizeOf(typeof(MBI));
        while (VirtualQueryEx(h, new IntPtr(addr), out m, new IntPtr(size)) != IntPtr.Zero) {
            long region = m.RegionSize.ToInt64();
            if (m.State == 0x1000) { // MEM_COMMIT
                int pages = (int)(region / 4096);
                var q = new WSEX[pages];
                for (int i = 0; i < pages; i++) q[i].Va = new IntPtr(addr + (long)i * 4096);
                long priv = 0;
                if (QueryWorkingSetEx(h, q, (uint)(pages * Marshal.SizeOf(typeof(WSEX))))) {
                    foreach (var p in q) {
                        long a = p.Attr.ToInt64();
                        bool valid = (a & 1) != 0, shared = (a & (1L << 15)) != 0;
                        // Privada para el Administrador de tareas: no compartible. Las páginas
                        // compartibles que solo usa este proceso (secciones del controlador de la
                        // GPU) se suman aparte.
                        if (valid && !shared) priv += 4096;
                        else if (valid && ((a >> 1) & 7) <= 1) soloAqui += 4096;
                    }
                }
                if (priv > 0) {
                    string key;
                    if (m.Type == 0x1000000 || m.Type == 0x40000) { // MEM_IMAGE o MEM_MAPPED
                        var sb = new StringBuilder(512);
                        GetMappedFileNameW(h, m.BaseAddress, sb, 512);
                        string name = sb.ToString(); int k = name.LastIndexOf('\\');
                        key = (m.Type == 0x1000000 ? "imagen " : "mapeado ") + (k >= 0 ? name.Substring(k + 1) : name);
                    } else {
                        key = "privada";
                        long ab = m.AllocationBase.ToInt64();
                        long v; byAlloc.TryGetValue(ab, out v); byAlloc[ab] = v + priv;
                    }
                    long w; byKey.TryGetValue(key, out w); byKey[key] = w + priv;
                    total += priv;
                }
            }
            if (m.State != 0x10000 || true) { // tamaño total de cada reserva
                long ab = m.AllocationBase.ToInt64(); long s; allocSize.TryGetValue(ab, out s); allocSize[ab] = s + region;
            }
            addr += region;
            if (addr <= 0) break;
        }
        CloseHandle(h);
        var lines = new List<string>();
        lines.Add(string.Format("Privada en memoria: {0:N1} MB (y {1:N1} MB compartibles que solo usa este proceso)", total / 1048576.0, soloAqui / 1048576.0));
        var keys = new List<KeyValuePair<string, long>>(byKey); keys.Sort((a, b) => b.Value.CompareTo(a.Value));
        for (int i = 0; i < Math.Min(top, keys.Count); i++)
            lines.Add(string.Format("  {0,8:N2} MB  {1}", keys[i].Value / 1048576.0, keys[i].Key));
        lines.Add("Reservas privadas más grandes (base, reservado, en memoria):");
        var allocs = new List<KeyValuePair<long, long>>(byAlloc); allocs.Sort((a, b) => b.Value.CompareTo(a.Value));
        for (int i = 0; i < Math.Min(top, allocs.Count); i++) {
            long res; allocSize.TryGetValue(allocs[i].Key, out res);
            lines.Add(string.Format("  {0:X12}  {1,9:N2} MB  {2,8:N2} MB", allocs[i].Key, res / 1048576.0, allocs[i].Value / 1048576.0));
        }
        return lines;
    }
}
'@
}
$tm = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$ProcessId").WorkingSetPrivate
"Administrador de tareas (conjunto de trabajo privado): {0:N1} MB" -f ($tm / 1MB)
[GarboMem]::Analyze($ProcessId, $Top)
