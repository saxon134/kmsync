param(
  [string]$Version = "0.1.0",
  [string]$Toolchain = "",
  [string]$Target = "x86_64-pc-windows-msvc",
  [string]$AuthenticodeCertificateThumbprint = "",
  [string]$PfxPath = "",
  [string]$PfxPassword = "",
  [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path "$PSScriptRoot\..\.."
$Dist = Join-Path $Root "dist\windows"
$Nsi = Join-Path $Root "packaging\windows\kmsync-daemon.nsi"
$ReleaseDir = Join-Path $Root "target\$Target\release"
$Installer = Join-Path $Dist "kmsync-daemon-$Version-windows-x64-setup.exe"

function Invoke-CheckedNative {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [string[]]$Arguments = @()
  )

  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "$FilePath failed with exit code $LASTEXITCODE"
  }
}

function Test-ShouldSignAuthenticode {
  return -not [string]::IsNullOrWhiteSpace($AuthenticodeCertificateThumbprint) -or
    -not [string]::IsNullOrWhiteSpace($PfxPath)
}

function Sign-AuthenticodeFile {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  if (-not (Test-ShouldSignAuthenticode)) {
    Write-Host "Authenticode signing not configured; leaving $Path unsigned"
    return
  }

  $signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if (-not $signtool) {
    throw "signtool.exe not found. Install Windows SDK signing tools."
  }

  $args = @("sign", "/fd", "SHA256", "/tr", $TimestampUrl, "/td", "SHA256")
  if (-not [string]::IsNullOrWhiteSpace($AuthenticodeCertificateThumbprint)) {
    $args += @("/sha1", $AuthenticodeCertificateThumbprint)
  } else {
    $args += @("/f", (Resolve-Path $PfxPath))
    if (-not [string]::IsNullOrWhiteSpace($PfxPassword)) {
      $args += @("/p", $PfxPassword)
    }
  }
  $args += $Path
  Invoke-CheckedNative -FilePath $signtool.Source -Arguments $args
}

New-Item -ItemType Directory -Force -Path $Dist | Out-Null

$cargoArgs = @()
if (-not [string]::IsNullOrWhiteSpace($Toolchain)) {
  $cargoArgs += "+$Toolchain"
}
$cargoArgs += @(
  "build",
  "--release",
  "-p",
  "kmsync-daemon",
  "--target",
  $Target
)
Invoke-CheckedNative -FilePath "cargo" -Arguments $cargoArgs

Sign-AuthenticodeFile (Join-Path $ReleaseDir "kmsync-daemon.exe")

$makensis = Get-Command makensis.exe -ErrorAction SilentlyContinue
if (-not $makensis) {
  throw "makensis.exe not found. Install NSIS first: choco install nsis"
}

Invoke-CheckedNative -FilePath $makensis.Source -Arguments @(
  "/DAPP_VERSION=$Version",
  "/DAPP_TARGET=$Target",
  $Nsi
)

Sign-AuthenticodeFile $Installer

Write-Host "Created Windows installer under $Dist"
