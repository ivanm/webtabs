# Installs the built MSI silently for the current user, checks what it put where,
# then uninstalls it and checks it's gone. Used by CI; safe to run in a test VM.
#   pwsh scripts/check-windows-msi.ps1
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$msi = (Get-Item target\dist\WebTabs-*-windows-x64.msi | Select-Object -First 1).FullName
$dir = "$env:LOCALAPPDATA\Programs\WebTabs"
$exe = "$dir\webtabs.exe"
$lnk = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\WebTabs.lnk"

function Msi([string]$verb, [string]$log) {
    $p = Start-Process msiexec.exe -ArgumentList "/$verb `"$msi`" /qn /norestart /l*v `"$log`"" -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        Get-Content $log -Tail 40
        throw "msiexec /$verb failed with $($p.ExitCode)"
    }
}

Msi i "$env:TEMP\webtabs-install.log"
foreach ($path in $exe, $lnk) {
    if (-not (Test-Path $path)) { throw "missing after install: $path" }
}
$target = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk).TargetPath
if ($target -ne $exe) { throw "shortcut points to $target" }
$fileVersion = (Get-Item $exe).VersionInfo.ProductVersion
# Per-user MSIs are registered under Windows Installer's UserData for the user's
# SID (what Apps & features lists), not under ...\CurrentVersion\Uninstall.
function Registered {
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    @(Get-ChildItem "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\$sid\Products" -ErrorAction SilentlyContinue |
        ForEach-Object { Get-ItemProperty "$($_.PSPath)\InstallProperties" -ErrorAction SilentlyContinue } |
        Where-Object DisplayName -eq 'WebTabs')
}
$product = Registered
if ($product.Count -ne 1) { throw "expected one registered WebTabs product, found $($product.Count)" }
"installed: $exe (file version $fileVersion), Apps & features: WebTabs $($product[0].DisplayVersion)"

Msi x "$env:TEMP\webtabs-uninstall.log"
foreach ($path in $exe, $lnk, $dir) {
    if (Test-Path $path) { throw "left behind after uninstall: $path" }
}
if ((Registered).Count) { throw "still registered after uninstall" }
"uninstalled cleanly"

$sums = Get-Content target\dist\SHA256SUMS-windows
foreach ($line in $sums) {
    $hash, $name = $line -split '  ', 2
    if ((Get-FileHash "target\dist\$name" -Algorithm SHA256).Hash.ToLower() -ne $hash) { throw "checksum mismatch: $name" }
}
"checksums ok"
