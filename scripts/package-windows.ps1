# Packages the Windows release into target\dist:
#   WebTabs-<version>-windows-x64.msi   per-user installer (WiX v5)
#   webtabs-<version>-windows-x64.zip   portable: webtabs.exe + README.md + LICENSE
#   SHA256SUMS-windows
# Needs the Rust MSVC toolchain, the Windows SDK and WiX:
#   dotnet tool install --global wix --version 5.0.2
#   pwsh scripts/package-windows.ps1 [-Version v0.2.0] [-SkipBuild]
# The version defaults to $env:WEBTABS_VERSION, then Cargo.toml's.
param([string]$Version = $env:WEBTABS_VERSION, [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if ($Version) {
    $Version = $Version -replace '^v', ''
} else {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
}
# MSI versions are numeric only: 0.2.0-test installs as 0.2.0.
$MsiVersion = ($Version -split '[-+]')[0]
if ($MsiVersion -notmatch '^\d+\.\d+\.\d+$') { throw "bad version: $Version" }

if (-not $SkipBuild) {
    cargo build --release --locked
    if ($LASTEXITCODE) { throw "cargo build failed" }
}
$exe = Resolve-Path target\release\webtabs.exe

$dist = 'target\dist'
New-Item -ItemType Directory -Force $dist | Out-Null
Get-ChildItem $dist -Include *windows*, SHA256SUMS-windows -Recurse | Remove-Item -Recurse -Force

$msi = "$dist\WebTabs-$Version-windows-x64.msi"
wix build packaging\windows\webtabs.wxs -arch x64 -d "Version=$MsiVersion" -d "Exe=$exe" `
    -d "Icon=$(Resolve-Path assets\windows\webtabs.ico)" -o $msi
if ($LASTEXITCODE) { throw "wix build failed" }
Remove-Item "$dist\*.wixpdb" -ErrorAction SilentlyContinue

$name = "webtabs-$Version-windows-x64"
$stage = "$dist\$name"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe, README.md $stage
if (Test-Path LICENSE) { Copy-Item LICENSE $stage }
$zip = "$dist\$name.zip"
Compress-Archive -Path $stage -DestinationPath $zip -Force
Remove-Item $stage -Recurse -Force

# sha256sum format with LF endings, so `sha256sum -c` works on any OS.
$sums = Get-ChildItem $msi, $zip | ForEach-Object {
    '{0}  {1}' -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name
}
[IO.File]::WriteAllText((Join-Path (Resolve-Path $dist) 'SHA256SUMS-windows'), ($sums -join "`n") + "`n")
Get-ChildItem $msi, $zip, "$dist\SHA256SUMS-windows" | ForEach-Object { $_.FullName }
