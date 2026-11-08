# Crea dist\openFatt-Setup-<versione>.exe. Servono Rust e NSIS 3 (winget install NSIS.NSIS).
# La versione è quella di [workspace.package] in Cargo.toml.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
if (-not (Test-Path $makensis)) { throw "NSIS non trovato: winget install NSIS.NSIS" }

$match = Select-String -Path Cargo.toml -Pattern '^version = "(\d+\.\d+\.\d+)"' | Select-Object -First 1
if (-not $match) { throw "Versione non trovata in Cargo.toml" }
$version = $match.Matches[0].Groups[1].Value

# Libreria C di Microsoft inclusa nel programma: deve partire anche su un Windows appena installato.
$env:RUSTFLAGS = "-C target-feature=+crt-static"
$env:CARGO_TARGET_DIR = Join-Path $root "target\dist"
cargo build --release -p openfatt
if ($LASTEXITCODE -ne 0) { throw "Compilazione non riuscita" }
# Lettore a 32 bit per importare gli archivi di Easyfatt: il motore Firebird di Easyfatt e' a 32 bit.
cargo build --release --target i686-pc-windows-msvc -p openfatt-fbread
if ($LASTEXITCODE -ne 0) { throw "Compilazione del lettore Easyfatt non riuscita" }

$exe = Join-Path $env:CARGO_TARGET_DIR "release\openfatt.exe"
$helper = Join-Path $env:CARGO_TARGET_DIR "i686-pc-windows-msvc\release\openfatt-easyfatt.exe"
foreach ($f in $exe, $helper) {
    if ([Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($f)) -match "(?i)vcruntime140") { throw "$f dipende ancora da VCRUNTIME140" }
}

# La pagina della licenza vuole gli a capo di Windows.
$license = Join-Path $env:CARGO_TARGET_DIR "LICENSE.txt"
[IO.File]::WriteAllText($license, ([IO.File]::ReadAllText((Join-Path $root "LICENSE")) -replace "\r?\n", "`r`n"), (New-Object Text.ASCIIEncoding))

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$out = Join-Path $dist "openFatt-Setup-$version.exe"
$icon = Join-Path $root "app\assets\openfatt.ico"

& $makensis /V2 /WX /INPUTCHARSET UTF8 "/DVERSION=$version" "/DEXE=$exe" "/DHELPER=$helper" "/DICON=$icon" "/DLICENSE=$license" "/DOUTFILE=$out" (Join-Path $root "tools\installatore.nsi")
if ($LASTEXITCODE -ne 0) { throw "Creazione dell'installatore non riuscita" }

# Firma: la chiave privata resta fuori dal progetto.
$chiave = if ($env:OPENFATT_CHIAVE_FIRMA) { $env:OPENFATT_CHIAVE_FIRMA } else { Join-Path (Split-Path -Parent $root) "chiave-aggiornamenti\openfatt-firma.key" }
if (-not (Test-Path $chiave)) { throw "Chiave di firma non trovata: $chiave" }
cargo run --release -p openfatt-update --bin openfatt-firma-rilascio -- firma $chiave $out $version (Join-Path $dist "openFatt-Update-$version.json")
if ($LASTEXITCODE -ne 0) { throw "Firma non riuscita" }

$hash = (Get-FileHash $out -Algorithm SHA256).Hash.ToLower()
"$hash  openFatt-Setup-$version.exe" | Set-Content -Encoding ascii "$out.sha256"
Write-Output "Creato $out"
Write-Output "SHA-256 $hash"
