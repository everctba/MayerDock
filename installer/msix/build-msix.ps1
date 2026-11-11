<#
.SYNOPSIS
    Empacota o MayerDock como MSIX (F1) — em paralelo ao instalador Inno.

.DESCRIPTION
    - Usa o payload já montado em dist\MayerDock\ (rode .\build.ps1 antes).
    - Gera AppxManifest.xml a partir de Package.appxmanifest.template.
    - Empacota com makeappx.exe (Windows SDK / SDK BuildTools) em dist\msix\.
    - Opcional: -SelfSign cria/usa um certificado de desenvolvimento
      (CN=MayerDock Dev) em CurrentUser, confia em TrustedPeople e assina;
      -Install instala via Add-AppxPackage (sideload); -Uninstall remove.

    NÃO altera dist\installer\ nem dist\MayerDock\.

.PARAMETER Version
    Versão do pacote (default: lida do Cargo.toml). Virá 4 partes no manifesto.

.PARAMETER IdentityName
    Identity Name (dev: MayerDock.Dev; Store: valor do Partner Center).

.PARAMETER Publisher
    Publisher do Identity (deve bater com o Subject do certificado que assina).

.PARAMETER SelfSign
    Cria/usa certificado de desenvolvimento CN=MayerDock Dev e assina o MSIX.

.PARAMETER Install
    Instala o MSIX gerado (sideload).

.PARAMETER Uninstall
    Remove pacotes instalados cujo nome começa com IdentityName.

.EXAMPLE
    .\installer\msix\build-msix.ps1 -SelfSign -Install
#>
param(
    [string]$Version = '',
    [string]$IdentityName = 'MayerDock.Dev',
    [string]$Publisher = 'CN=MayerDock Dev',
    [string]$PublisherDisplayName = 'MayerDock',
    [switch]$SelfSign,
    [switch]$Install,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$payload = Join-Path $root 'dist\MayerDock'
$outDir = Join-Path $root 'dist\msix'
$stage = Join-Path $outDir 'stage'

# Instalar por sideload exige assinatura: -Install implica -SelfSign.
if ($Install) { $SelfSign = $true }

if (-not (Test-Path (Join-Path $payload 'mayerdock.exe'))) {
    throw "payload ausente: rode '.\build.ps1' antes (dist\MayerDock\mayerdock.exe nao existe)."
}

# ---------------------------------------------------------------- versão
if (-not $Version) {
    $line = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^\s*version\s*=\s*"([^"]+)"' |
        Select-Object -First 1
    if (-not $line) { throw 'nao foi possivel ler a versao do Cargo.toml' }
    $Version = $line.Matches[0].Groups[1].Value
}
$parts = $Version.Split('.')
while ($parts.Count -lt 4) { $parts += '0' }
$ver4 = ($parts[0..3] -join '.')

# ---------------------------------------------------------------- ferramentas
function Find-SdkTool {
    param([string]$Name, [string]$EnvVar)
    if ($EnvVar) {
        $fromEnv = [Environment]::GetEnvironmentVariable($EnvVar)
        if ($fromEnv -and (Test-Path $fromEnv)) { return $fromEnv }
    }
    $candidates = @()
    $kits = @(
        'C:\Program Files (x86)\Windows Kits\10\bin',
        'C:\Program Files\Windows Kits\10\bin'
    )
    foreach ($k in $kits) {
        if (Test-Path $k) {
            $candidates += Get-ChildItem $k -Filter $Name -Recurse -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -ExpandProperty FullName
        }
    }
    $localTools = Join-Path $root '.tools'
    if (Test-Path $localTools) {
        $candidates += Get-ChildItem $localTools -Filter $Name -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -ExpandProperty FullName
    }
    $fromPath = (Get-Command $Name -ErrorAction SilentlyContinue).Source
    if ($fromPath) { $candidates += $fromPath }
    return ($candidates | Select-Object -First 1)
}

$makeappx = Find-SdkTool -Name 'makeappx.exe' -EnvVar 'MAKEAPPX'
if (-not $makeappx) {
    throw @'
makeappx.exe nao encontrado.
Opcoes (sem admin):
  1. Baixe o pacote NuGet Microsoft.Windows.SDK.BuildTools e extraia em .tools\win-sdk-buildtools\
     https://www.nuget.org/packages/Microsoft.Windows.SDK.BuildTools
  2. Ou instale o Windows SDK; ou defina MAKEAPPX=...\makeappx.exe
'@
}

# ---------------------------------------------------------------- staging
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'Assets') | Out-Null
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Copy-Item -LiteralPath (Join-Path $payload 'mayerdock.exe') -Destination $stage
Copy-Item -Path (Join-Path $PSScriptRoot 'Assets\*') -Destination (Join-Path $stage 'Assets')

$template = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'Package.appxmanifest.template') -Raw -Encoding UTF8
$manifest = $template.Replace('@@IDENTITY_NAME@@', $IdentityName)
$manifest = $manifest.Replace('@@PUBLISHER@@', $Publisher)
$manifest = $manifest.Replace('@@VERSION@@', $ver4)
$manifest = $manifest.Replace('@@PUBLISHER_DISPLAY@@', $PublisherDisplayName)
Set-Content -LiteralPath (Join-Path $stage 'AppxManifest.xml') -Value $manifest -Encoding UTF8

# ---------------------------------------------------------------- pack
$msix = Join-Path $outDir ("MayerDock-$Version.msix")
Write-Host "[msix] makeappx pack -> $msix" -ForegroundColor DarkGray
& $makeappx pack /d $stage /p $msix /o
if ($LASTEXITCODE -ne 0) { throw "makeappx falhou (exit $LASTEXITCODE)." }
Write-Host "ok: $msix ($([math]::Round((Get-Item $msix).Length/1MB,2)) MB)" -ForegroundColor Green

# ---------------------------------------------------------------- assinatura
if ($SelfSign) {
    $signtool = Find-SdkTool -Name 'signtool.exe' -EnvVar 'SIGNTOOL'
    if (-not $signtool) { throw 'signtool.exe nao encontrado (necessario para -SelfSign).' }
    if ($Publisher -notmatch '^CN=') {
        throw "para -SelfSign, -Publisher deve ser um Subject CN=... (ex.: 'CN=MayerDock Dev')."
    }

    $cert = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $Publisher -and $_.HasPrivateKey } |
        Select-Object -First 1
    if (-not $cert) {
        Write-Host "[msix] criando certificado de desenvolvimento: $Publisher" -ForegroundColor DarkGray
        $cert = New-SelfSignedCertificate `
            -Type CodeSigningCert `
            -Subject $Publisher `
            -KeyUsage DigitalSignature `
            -FriendlyName 'MayerDock Dev' `
            -CertStoreLocation Cert:\CurrentUser\My `
            -NotAfter (Get-Date).AddYears(3)
    }

    $cerPath = Join-Path $outDir 'mayerdock-dev.cer'
    Export-Certificate -Cert $cert -FilePath $cerPath -Force | Out-Null
    Import-Certificate -FilePath $cerPath -CertStoreLocation Cert:\CurrentUser\TrustedPeople | Out-Null
    Write-Host "[msix] confiando no certificado de dev em CurrentUser\TrustedPeople" -ForegroundColor DarkGray

    & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /d "MayerDock" $msix
    if ($LASTEXITCODE -ne 0) { throw "signtool falhou (exit $LASTEXITCODE)." }
    Write-Host "assinado: $msix" -ForegroundColor Green
}
elseif (-not $Install) {
    Write-Host 'aviso: pacote sem assinatura (use -SelfSign para sideload local; a Store assina na publicacao).' -ForegroundColor Yellow
}

# ---------------------------------------------------------------- instalar
if ($Install) {
    Write-Host '[msix] Add-AppxPackage...' -ForegroundColor DarkGray
    Add-AppxPackage -Path $msix
    $pkg = Get-AppxPackage -Name "$IdentityName*" | Select-Object -First 1
    if ($pkg) {
        Write-Host "instalado: $($pkg.PackageFullName)" -ForegroundColor Green
        Write-Host "pasta: $($pkg.InstallLocation)" -ForegroundColor DarkGray
    }
}

if ($Uninstall) {
    $pkgs = Get-AppxPackage -Name "$IdentityName*"
    foreach ($p in $pkgs) { Remove-AppxPackage -Package $p.PackageFullName }
    if ($pkgs) { Write-Host "removido(s): $($pkgs.Count) pacote(s)" -ForegroundColor Green }
}
