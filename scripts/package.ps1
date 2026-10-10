# Construit une version distribuable de Mudra dans dist\ :
#   -Mode zip : Mudra-<version>.zip  (mudra.exe + models\ + DLL eventuelles)
#   -Mode exe : mudra-<version>.exe  (un seul fichier, modeles embarques)
#
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1 -Mode zip
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1 -Mode exe

param([ValidateSet('zip', 'exe')] [string] $Mode = 'zip')

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

& powershell -ExecutionPolicy Bypass -File scripts\fetch-models.ps1
if ($LASTEXITCODE) { exit 1 }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$target = Join-Path $root 'target'
$features = if ($Mode -eq 'exe') { @('--features', 'embed-models') } else { @() }
& cargo build --release -p mudra-app @features
if ($LASTEXITCODE) { exit 1 }

$release = Join-Path $target 'release'
$dlls = @(Get-ChildItem $release -Filter *.dll)
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null

if ($Mode -eq 'exe') {
    # DirectML.dll fait partie de Windows 10 1903+ ; a defaut, repli sur CPU (D-07).
    $others = @($dlls | Where-Object Name -ne 'DirectML.dll')
    if ($others) {
        Write-Host "Le binaire depend de DLL ($($others.Name -join ', ')) : utilise -Mode zip." -ForegroundColor Red
        exit 1
    }
    $out = Join-Path $dist "mudra-$version.exe"
    Copy-Item (Join-Path $release 'mudra.exe') $out -Force
} else {
    $stage = Join-Path $target "Mudra-$version"
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force (Join-Path $stage 'models') | Out-Null
    Copy-Item (Join-Path $release 'mudra.exe') $stage
    $dlls | Copy-Item -Destination $stage
    Copy-Item models\*.onnx, models\LICENSE, models\NOTICE (Join-Path $stage 'models')
    Copy-Item README.md $stage
    $out = Join-Path $dist "Mudra-$version.zip"
    Compress-Archive -Path $stage -DestinationPath $out -Force
}
Write-Host ("OK  {0}  ({1:N0} octets)" -f $out, (Get-Item $out).Length)
