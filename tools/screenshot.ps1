# Avvia l'app su una schermata, cattura la finestra in PNG e chiude il processo.
# Uso: tools\screenshot.ps1 -Archive <file.gadb> -Screen <nome> -Out <file.png> [-Click "x,y"]
# -Click invia un clic alla sola finestra dell'app (coordinate dell'area client in pixel fisici);
# -Type precompila la ricerca generale (variabile OPENFATT_SEARCH).
param([string]$Archive, [string]$Screen = "", [string]$Out, [int]$Wait = 4, [string]$Click = "", [string]$Type = "", [string]$Clicks = "")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W32 {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
"@
[void][W32]::SetProcessDPIAware()
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\openfatt.exe'
if ($env:OPENFATT_EXE) { $exe = $env:OPENFATT_EXE }
$env:OPENFATT_ARCHIVE = $Archive
$env:OPENFATT_SCREEN = $Screen
$env:OPENFATT_SEARCH = $Type
$env:OPENFATT_CLICKS = $Clicks
$p = Start-Process -FilePath $exe -PassThru
try {
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 40 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 250; $p.Refresh(); $h = $p.MainWindowHandle }
  if ($h -eq [IntPtr]::Zero) { throw "finestra non trovata" }
  [void][W32]::ShowWindow($h, 9)
  [void][W32]::SetForegroundWindow($h)
  Start-Sleep -Seconds $Wait
  # con archivi grandi la prima finestra trovata può essere provvisoria: si rilegge
  $p.Refresh(); if ($p.MainWindowHandle -ne [IntPtr]::Zero) { $h = $p.MainWindowHandle }
  if ($Click -ne "") {
    $xy = $Click.Split(","); $lp = [IntPtr](([int]$xy[1] -shl 16) -bor ([int]$xy[0] -band 0xFFFF))
    [void][W32]::PostMessage($h, 0x200, [IntPtr]0, $lp)
    Start-Sleep -Milliseconds 300
    [void][W32]::PostMessage($h, 0x201, [IntPtr]1, $lp)
    Start-Sleep -Milliseconds 80
    [void][W32]::PostMessage($h, 0x202, [IntPtr]0, $lp)
    Start-Sleep -Seconds 1
  }
  $r = New-Object W32+RECT
  [void][W32]::GetWindowRect($h, [ref]$r)
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  # cattura solo la finestra dell'app (anche se coperta da altre finestre)
  $hdc = $g.GetHdc()
  [void][W32]::PrintWindow($h, $hdc, 2)
  $g.ReleaseHdc($hdc)
  $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  Write-Output "salvato $Out ($w x $hh)"
} finally {
  if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}
