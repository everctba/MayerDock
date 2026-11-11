<#
    Compila o MayerDock e monta a pasta portátil dist\MayerDock\.

    Usa o toolchain MSVC padrão quando existe link.exe; senão usa
    x86_64-pc-windows-gnu do rustup + o primeiro gcc x86_64 encontrado
    (w64devkit/MSYS2/MinGW-w64 no PATH ou em pastas conhecidas).

    No w64devkit o libgcc_eh.a pode faltar; como no FluxTV, criamos um stub
    vazio e apontamos o linker para ele via -L native=...

    Uso:
        .\build.ps1              # compila e monta dist\MayerDock\
        .\build.ps1 -Run         # + abre o .exe da pasta portátil
        .\build.ps1 -Shortcut    # + atalho na área de trabalho
        .\build.ps1 -Installer   # + instalador (Inno Setup) em dist\installer\
        .\build.ps1 -Msix        # + pacote MSIX em dist\msix\ (não substitui o instalador)
#>
param(
    [switch]$Run,
    [switch]$Shortcut,
    [switch]$Installer,
    [switch]$Msix
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
Push-Location $root

$origLinker = $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER
$origRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
$origPath = $env:PATH
$origErrorAction = $ErrorActionPreference

function Find-X64Gcc {
    $candidates = [System.Collections.Generic.List[string]]::new()
    $names = @('x86_64-w64-mingw32-gcc.exe', 'gcc.exe')

    if ($env:MINGW64_BIN -and (Test-Path $env:MINGW64_BIN)) {
        foreach ($n in $names) {
            $p = Join-Path $env:MINGW64_BIN $n
            if (Test-Path $p) { $candidates.Add($p) }
        }
    }
    foreach ($entry in ($env:PATH -split ';')) {
        if (-not $entry -or -not (Test-Path $entry)) { continue }
        foreach ($n in $names) {
            $p = Join-Path $entry $n
            if (Test-Path $p) { $candidates.Add($p) }
        }
    }
    $roots = @(
        'I:\P\Vim\w64devkit\bin', 'C:\w64devkit\bin', 'D:\w64devkit\bin',
        'C:\msys64\mingw64\bin', 'C:\msys64\ucrt64\bin', 'D:\msys64\mingw64\bin',
        'C:\mingw64\bin', 'C:\mingw-w64\bin', 'C:\tools\mingw64\bin',
        'D:\mingw64\bin', 'D:\mingw-w64\bin'
    )
    foreach ($r in $roots) {
        if (-not (Test-Path $r)) { continue }
        foreach ($n in $names) {
            $p = Join-Path $r $n
            if (Test-Path $p) { $candidates.Add($p) }
        }
    }

    foreach ($gcc in ($candidates | Select-Object -Unique)) {
        try { $machine = (& $gcc -dumpmachine 2>$null | Out-String).Trim() } catch { continue }
        if ($machine -match '^x86_64') { return $gcc }
    }
    return $null
}

try {
    # stderr do cargo (progresso) não pode virar erro terminante no PowerShell 5.1
    $ErrorActionPreference = 'Continue'

    if (Get-Command link.exe -ErrorAction SilentlyContinue) {
        & cargo build --release --offline
    }
    else {
        $gcc = Find-X64Gcc
        if (-not $gcc) {
            throw 'Nenhum gcc x86_64 encontrado (w64devkit/MSYS2/MinGW-w64) nem link.exe do MSVC.'
        }
        $gnu = (& rustup toolchain list 2>$null) | Where-Object { $_ -match 'windows-gnu' } | Select-Object -First 1
        if (-not $gnu) {
            throw 'Toolchain x86_64-pc-windows-gnu não instalado (rustup toolchain install stable-x86_64-pc-windows-gnu).'
        }
        $gnu = ($gnu -split '\s+')[0]
        $binDir = Split-Path $gcc -Parent
        $env:PATH = "$binDir;$env:PATH"
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $gcc

        # libgcc_eh.a ausente (w64devkit): stub vazio + -L native.
        $gccLib = (& $gcc -print-libgcc-file-name 2>$null | Out-String).Trim()
        $libDir = if ($gccLib) { Split-Path $gccLib -Parent } else { '' }
        $hasEh = $false
        if ($libDir) {
            $hasEh = (Test-Path (Join-Path $libDir 'libgcc_eh.a')) -or
                     (Test-Path (Join-Path $libDir '..\libgcc_eh.a'))
        }
        if (-not $hasEh) {
            $stubDir = Join-Path $root 'target\toolchain-stub'
            New-Item -ItemType Directory -Force -Path $stubDir | Out-Null
            $stub = Join-Path $stubDir 'libgcc_eh.a'
            if (-not (Test-Path $stub)) {
                $ar = $null
                foreach ($n in @('x86_64-w64-mingw32-ar.exe', 'ar.exe', 'llvm-ar.exe')) {
                    $p = Join-Path $binDir $n
                    if (Test-Path $p) { $ar = $p; break }
                }
                if (-not $ar) { throw 'libgcc_eh.a ausente e ar.exe não encontrado para criar o stub.' }
                & $ar rcs $stub 2>$null | Out-Null
            }
            $sep = [char]0x1f
            $parts = @()
            if ($env:CARGO_ENCODED_RUSTFLAGS) { $parts += $env:CARGO_ENCODED_RUSTFLAGS }
            $parts += @('-L', "native=$stubDir")
            $env:CARGO_ENCODED_RUSTFLAGS = ($parts -join $sep)
        }

        Write-Host "[mayerdock] cargo +$gnu build --release --offline (gcc: $gcc)" -ForegroundColor DarkGray
        & cargo "+$gnu" build --release --offline 2>&1 | ForEach-Object { Write-Host $_ }
    }

    if ($LASTEXITCODE -ne 0) { throw "cargo falhou (exit $LASTEXITCODE)." }

    $exe = Join-Path $root 'target\release\mayerdock.exe'
    if (-not (Test-Path $exe)) { throw "binário não gerado: $exe" }
    Write-Host "OK: $exe" -ForegroundColor Green

    # ------------------------------------------------------------------
    # Pasta portátil: dist\MayerDock\ com .exe + docs
    # ------------------------------------------------------------------
    $dist = Join-Path $root 'dist\MayerDock'
    if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    Copy-Item -LiteralPath $exe (Join-Path $dist 'mayerdock.exe')
    foreach ($doc in @('README.md', 'LICENSE')) {
        $src = Join-Path $root $doc
        if (Test-Path $src) { Copy-Item -LiteralPath $src $dist }
    }
    $docsDir = Join-Path $root 'docs'
    if (Test-Path $docsDir) {
        Copy-Item -LiteralPath $docsDir (Join-Path $dist 'docs') -Recurse
    }
    $portableExe = Join-Path $dist 'mayerdock.exe'
    Write-Host "portátil: $dist" -ForegroundColor Green

    # ------------------------------------------------------------------
    # Atalho opcional na área de trabalho
    # ------------------------------------------------------------------
    if ($Shortcut) {
        $desktop = [Environment]::GetFolderPath('Desktop')
        $lnk = Join-Path $desktop 'MayerDock.lnk'
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($lnk)
        $shortcut.TargetPath = $portableExe
        $shortcut.WorkingDirectory = $dist
        $shortcut.Description = 'MayerDock — dock lateral'
        $shortcut.Save()
        Write-Host "atalho: $lnk" -ForegroundColor Green
    }

    # ------------------------------------------------------------------
    # Instalador (Inno Setup) — opcional via -Installer
    # ------------------------------------------------------------------
    if ($Installer) {
        $iscc = $env:INNO_SETUP_ISCC
        if (-not $iscc) {
            $candidates = @(
                (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
                (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 5\ISCC.exe'),
                'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
                'C:\Program Files\Inno Setup 6\ISCC.exe'
            )
            foreach ($c in $candidates) {
                if (Test-Path $c) { $iscc = $c; break }
            }
        }
        if (-not $iscc -or -not (Test-Path $iscc)) {
            Write-Host 'aviso: Inno Setup (ISCC.exe) nao encontrado; instalador nao gerado.' -ForegroundColor Yellow
            Write-Host '  instale sem admin: baixe https://jrsoftware.org/isdl.php e rode:'
            Write-Host '    innosetup-x.y.z.exe /VERYSILENT /CURRENTUSER /SUPPRESSMSGBOXES /NORESTART'
            Write-Host '  ou aponte a variavel INNO_SETUP_ISCC para o ISCC.exe.'
        }
        else {
            $iss = Join-Path $root 'installer\MayerDock.iss'
            if (-not (Test-Path $iss)) { throw "script do instalador nao encontrado: $iss" }
            Write-Host "[mayerdock] ISCC: $iscc" -ForegroundColor DarkGray
            & $iscc '/Qp' $iss
            if ($LASTEXITCODE -ne 0) { throw "ISCC falhou (exit $LASTEXITCODE)." }
            $installer = Get-ChildItem (Join-Path $root 'dist\installer') -Filter 'MayerDock-Setup-*.exe' -ErrorAction SilentlyContinue |
                Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($installer) {
                Write-Host "instalador: $($installer.FullName)" -ForegroundColor Green
            }
        }
    }

    # ------------------------------------------------------------------
    # MSIX — opcional via -Msix (em paralelo ao instalador Inno)
    # ------------------------------------------------------------------
    if ($Msix) {
        $msixScript = Join-Path $root 'installer\msix\build-msix.ps1'
        if (-not (Test-Path $msixScript)) { throw "script MSIX nao encontrado: $msixScript" }
        Write-Host '[mayerdock] gerando MSIX...' -ForegroundColor DarkGray
        & $msixScript
        $msixFile = Get-ChildItem (Join-Path $root 'dist\msix') -Filter 'MayerDock-*.msix' -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($msixFile) { Write-Host "msix: $($msixFile.FullName)" -ForegroundColor Green }
    }

    if ($Run) { Start-Process -FilePath $portableExe }
}
finally {
    $env:PATH = $origPath
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $origLinker
    $env:CARGO_ENCODED_RUSTFLAGS = $origRustFlags
    $ErrorActionPreference = $origErrorAction
    Pop-Location
}
