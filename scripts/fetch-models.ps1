# Telecharge les modeles de vision MediaPipe Hands (D-05) depuis opencv_zoo
# et verifie leur empreinte SHA-256. C'est le seul acces reseau du projet :
# l'application elle-meme ne se connecte jamais (ENF-20).
# (Script volontairement en ASCII : Windows PowerShell 5.1 lit les .ps1 en ANSI.)
#
#   powershell -ExecutionPolicy Bypass -File scripts\fetch-models.ps1

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$models = Join-Path $root 'models'
New-Item -ItemType Directory -Force $models | Out-Null

$base = 'https://github.com/opencv/opencv_zoo/raw/main/models'
$files = @(
    @{ Name = 'palm_detection_mediapipe_2023feb.onnx';      Url = "$base/palm_detection_mediapipe/palm_detection_mediapipe_2023feb.onnx" },
    @{ Name = 'handpose_estimation_mediapipe_2023feb.onnx'; Url = "$base/handpose_estimation_mediapipe/handpose_estimation_mediapipe_2023feb.onnx" },
    @{ Name = 'LICENSE';                                    Url = "$base/palm_detection_mediapipe/LICENSE" }
)

# Empreintes attendues : models/SHA256SUMS (format "<sha256>  <nom>").
$expected = @{}
$sumsPath = Join-Path $models 'SHA256SUMS'
if (Test-Path $sumsPath) {
    foreach ($line in Get-Content $sumsPath) {
        if ($line -match '^([0-9a-fA-F]{64})\s+(.+)$') { $expected[$Matches[2].Trim()] = $Matches[1].ToLower() }
    }
}

$failed = $false
foreach ($f in $files) {
    $dest = Join-Path $models $f.Name
    if (-not (Test-Path $dest)) {
        Write-Host "Telechargement de $($f.Name)..."
        Invoke-WebRequest -Uri $f.Url -OutFile $dest -UseBasicParsing
    }
    $hash = (Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower()
    $size = (Get-Item $dest).Length
    if ($expected.ContainsKey($f.Name)) {
        if ($expected[$f.Name] -eq $hash) {
            Write-Host ("OK      {0}  ({1:N0} octets)" -f $f.Name, $size)
        } else {
            Write-Host ("ERREUR  {0} : empreinte {1}, attendue {2}" -f $f.Name, $hash, $expected[$f.Name]) -ForegroundColor Red
            $failed = $true
        }
    } else {
        Write-Host ("NOUVEAU {0}  ({1:N0} octets)  sha256 {2}" -f $f.Name, $size, $hash)
    }
}

if ($failed) {
    Write-Host "Au moins un fichier ne correspond pas a models/SHA256SUMS : supprime-le et relance." -ForegroundColor Red
    exit 1
}
