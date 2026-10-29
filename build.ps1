<#
    Compila o MayerDock.

    Usa o toolchain MSVC padrão quando existe link.exe; senão usa
    x86_64-pc-windows-gnu do rustup + o primeiro gcc x86_64 encontrado
    (w64devkit/MSYS2/MinGW-w64 no PATH ou em pastas conhecidas).

    No w64devkit o libgcc_eh.a pode faltar; como no FluxTV, criamos um stub
    vazio e apontamos o linker para ele via -L native=...
#>
param([switch]$Run)

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

    if ($Run) { Start-Process $exe }
}
finally {
    $env:PATH = $origPath
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $origLinker
    $env:CARGO_ENCODED_RUSTFLAGS = $origRustFlags
    $ErrorActionPreference = $origErrorAction
    Pop-Location
}
