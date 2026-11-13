# Capture the agent window via PrintWindow (works when the desktop session is locked,
# unlike CopyFromScreen which fails with a Win32Exception).
# ASCII-only: Windows PowerShell 5.1 reads .ps1 as ANSI.
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
[void][Win32]::SetProcessDPIAware()

$proc = Get-Process tl-agent -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { throw 'no tl-agent window' }
$h = $proc.MainWindowHandle

[void][Win32]::ShowWindow($h, 9)
[void][Win32]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 900

$rect = New-Object Win32+RECT
[void][Win32]::GetWindowRect($h, [ref]$rect)
$w = $rect.Right - $rect.Left
$ht = $rect.Bottom - $rect.Top

$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# 2 = PW_RENDERFULLCONTENT（WebView2 内容需要）
[void][Win32]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
$out = Join-Path $env:TEMP 'tl-ui.png'
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
Write-Host "window ${w}x${ht} -> $out"
