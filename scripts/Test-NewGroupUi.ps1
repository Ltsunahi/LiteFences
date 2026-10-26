# 新建分组 / 输入对话框回归：点「＋ 新建分组」→ 填名字 → 确定，
# 验证主进程不退出、分组写进 groups.txt，且连续两次都能用。
#
# 用法（在已构建并部署到 E:\note\LiteFencesDesktop 之后）：
#   pwsh -File scripts\Test-NewGroupUi.ps1
# 依赖：以当前用户身份、能访问交互桌面；数据写到独立目录，不碰用户配置。
param(
    [string]$Exe = 'E:\note\LiteFencesDesktop\LiteFencesDesktop.exe',
    [string]$DataDir = 'E:\note\.build\ui-test',
    [string]$Instance = '-uitest',
    [string[]]$Names = @('自动测试A', '自动测试B'),
    [string]$ProgressLog = 'E:\note\.build\ui-progress.txt'
)

$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class U {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendMessageText(IntPtr h, int msg, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
"@

function Say([string]$text) { Add-Content -Path $ProgressLog -Value $text -Encoding UTF8 }

function Find-Top([string]$cls) {
    $script:hit = [IntPtr]::Zero
    $cb = [U+EnumProc]{
        param($h, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [U]::GetClassName($h, $sb, 256) | Out-Null
        if ($sb.ToString() -eq $cls) { $script:hit = $h; return $false }
        return $true
    }
    [U]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
    return $script:hit
}

function Find-Child([IntPtr]$parent, [string]$cls) {
    $script:child = [IntPtr]::Zero
    $cb = [U+EnumProc]{
        param($h, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [U]::GetClassName($h, $sb, 256) | Out-Null
        if ($sb.ToString() -eq $cls) { $script:child = $h; return $false }
        return $true
    }
    [U]::EnumChildWindows($parent, $cb, [IntPtr]::Zero) | Out-Null
    return $script:child
}

function Wait-Window([string]$cls, [int]$timeoutMs) {
    $deadline = (Get-Date).AddMilliseconds($timeoutMs)
    while ((Get-Date) -lt $deadline) {
        $h = Find-Top $cls
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 100
    }
    return [IntPtr]::Zero
}

Remove-Item $ProgressLog -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $DataDir | Out-Null
$env:LITEFENCES_DESKTOP_LOG_DIR = $DataDir
$env:LITEFENCES_DESKTOP_INSTANCE = $Instance
$log = Join-Path $DataDir 'launcher.log'

Get-Process LiteFencesDesktop -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500
& $Exe
Say 'launched'
Start-Sleep -Milliseconds 1500

$proc = Get-Process LiteFencesDesktop -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Say 'fail: 进程没有起来'; exit 2 }
Say "pid=$($proc.Id)"

$host_wnd = Wait-Window 'LiteFencesDesktopHost' 8000
if ($host_wnd -eq [IntPtr]::Zero) { Say 'fail: 找不到宿主窗口'; exit 3 }
Say "host=$host_wnd"

foreach ($name in $Names) {
    $overlay = Find-Top 'LiteFencesOverlay'
    if ($overlay -eq [IntPtr]::Zero -or -not [U]::IsWindowVisible($overlay)) {
        [U]::PostMessage($host_wnd, 0x0312, [IntPtr]1, [IntPtr]::Zero) | Out-Null
        Start-Sleep -Milliseconds 2500
        $overlay = Find-Top 'LiteFencesOverlay'
    }
    if ($overlay -eq [IntPtr]::Zero) { Say 'fail: 面板窗口没有出现'; exit 4 }

    $text = Get-Content $log -Raw
    $found = [regex]::Matches($text, '唤出面板：.*?@(\d+)dpi')
    if ($found.Count -eq 0) { Say 'fail: 日志里没有布局信息'; exit 5 }
    $dpi = [int]$found[$found.Count - 1].Groups[1].Value
    $low = $text -match '以低完整性级别运行'
    $scale = $dpi / 96.0
    $barTop = if ($low) { 40 * $scale } else { 0 }
    $x = [int]((120 + 62) * $scale)
    $y = [int]($barTop + 23 * $scale)
    # 跨 DPI 感知上下文投递鼠标消息时，系统会按目标窗口 DPI 缩放 lparam，这里先折算回去。
    $vx = [int][math]::Round($x * 96.0 / $dpi)
    $vy = [int][math]::Round($y * 96.0 / $dpi)
    $lparam = (($vy -shl 16) -bor ($vx -band 0xFFFF))
    Say "click chip client=($x,$y) posted=($vx,$vy) dpi=$dpi low=$low"
    [U]::PostMessage($overlay, 0x0201, [IntPtr]0, [IntPtr]$lparam) | Out-Null

    $dialog = Wait-Window 'LiteFencesTextInput' 5000
    if ($dialog -eq [IntPtr]::Zero) { Say 'fail: 输入对话框没有出现'; exit 6 }

    $edit = Find-Child $dialog 'Edit'
    if ($edit -eq [IntPtr]::Zero) { Say 'fail: 找不到输入框'; exit 7 }
    [U]::SendMessageText($edit, 0x000C, [IntPtr]::Zero, $name) | Out-Null
    [U]::PostMessage($dialog, 0x0111, [IntPtr]3002, [IntPtr]::Zero) | Out-Null

    Start-Sleep -Milliseconds 1200
    if (-not (Get-Process -Id $proc.Id -ErrorAction SilentlyContinue)) {
        Say "FAIL: 点完确定后进程退出了（新建分组「$name」）"
        exit 8
    }
    $text = Get-Content $log -Raw
    if ($text -match [regex]::Escape("新建分组「$name」")) {
        Say "ok: 新建分组「$name」，进程仍在运行"
    } else {
        Say "FAIL: 日志里没有新建分组「$name」"
        exit 9
    }
}

Say '--- groups.txt'
Say ((Get-Content (Join-Path $DataDir 'groups.txt') -Raw))
[U]::PostMessage($host_wnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Milliseconds 800
$alive = [bool](Get-Process -Id $proc.Id -ErrorAction SilentlyContinue)
Say "final_alive=$alive (关闭后应为 False)"
Say 'PASS'
exit 0
