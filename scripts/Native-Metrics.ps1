# Read-only counters; API layout matches PROCESS_MEMORY_COUNTERS_EX2 from psapi.h.
if (-not ('LiteFences.BenchCounters' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
namespace LiteFences {
    public static class BenchCounters {
        [StructLayout(LayoutKind.Sequential)]
        public struct Memory {
            public uint cb, PageFaultCount;
            public UIntPtr PeakWorkingSetSize, WorkingSetSize;
            public UIntPtr QuotaPeakPagedPoolUsage, QuotaPagedPoolUsage;
            public UIntPtr QuotaPeakNonPagedPoolUsage, QuotaNonPagedPoolUsage;
            public UIntPtr PagefileUsage, PeakPagefileUsage, PrivateUsage;
            public UIntPtr PrivateWorkingSetSize;
            public ulong SharedCommitUsage;
        }
        [DllImport("psapi.dll", SetLastError=true)]
        private static extern bool GetProcessMemoryInfo(IntPtr process, ref Memory counters, uint size);
        [DllImport("kernel32.dll", SetLastError=true)]
        public static extern uint GetActiveProcessorCount(ushort group);
        [DllImport("user32.dll", SetLastError=true)]
        public static extern uint GetGuiResources(IntPtr process, uint flags);
        [DllImport("user32.dll")]
        public static extern bool IsIconic(IntPtr window);
        [DllImport("user32.dll")]
        public static extern bool IsWindowVisible(IntPtr window);
        public static Memory Read(IntPtr process) {
            var result = new Memory();
            result.cb = (uint)Marshal.SizeOf(typeof(Memory));
            if (!GetProcessMemoryInfo(process, ref result, result.cb))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (result.PrivateWorkingSetSize.ToUInt64() > result.WorkingSetSize.ToUInt64())
                throw new InvalidOperationException("Private working set exceeds total working set.");
            return result;
        }
    }
}
'@
}
