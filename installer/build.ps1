# Compila Rust Sonar en release y genera el instalador en dist\RustSonar-Instalador-<versión>-USO-PERSONAL.exe:
# la interfaz propia (installer\ui) con el instalador de Inno Setup (installer\RustSonar.iss) dentro.
# Uso: powershell -ExecutionPolicy Bypass -File installer\build.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "No encuentro Inno Setup 6. Instálalo con: winget install JRSoftware.InnoSetup --scope user" }

foreach ($f in 'instaladores\VBCABLE_Driver_Pack46\VBCABLE_Setup_x64.exe',
               'instaladores\HiFiCableAsioBridgeSetup_v1007\HiFiCableAsioBridgeSetup.exe',
               'instaladores\Voicemeeter8Setup_v3130\Voicemeeter8Setup.exe') {
    if (-not (Test-Path $f)) { throw "Falta $f (paquetes de vb-audio.com descomprimidos en instaladores\)" }
}

# Versión de Cargo.toml.
$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build falló" }

# Firma con el certificado "kilorito" (installer\certificado.ps1): el .exe, el instalador y el desinstalador.
$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object { $_.Subject -eq 'CN=kilorito' -and $_.NotAfter -gt (Get-Date) } |
    Sort-Object NotAfter -Descending | Select-Object -First 1
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
$isccArgs = @("/DAppVersion=$version")
if ($cert -and $signtool) {
    & $signtool sign /sha1 $cert.Thumbprint /s My /fd SHA256 /d "Rust Sonar" target\release\rust_sonar.exe
    if ($LASTEXITCODE -ne 0) { throw "no se pudo firmar rust_sonar.exe" }
    # Inno Setup llama a esta orden para firmar setup y desinstalador ($f = archivo, $q = comillas).
    $isccArgs += "/Skilorito=`$q$signtool`$q sign /sha1 $($cert.Thumbprint) /s My /fd SHA256 /d `$qRust Sonar`$q `$f"
    $isccArgs += "/DSign"
} else {
    Write-Warning "Sin certificado 'kilorito' o sin signtool: se genera sin firmar (ejecuta installer\certificado.ps1)."
}

# 1) Instalador de Inno (el que hace el trabajo), en dist\interno.
$inner = "RustSonar-Inno-$version"
& $iscc @isccArgs "/O$root\dist\interno" "/F$inner" installer\RustSonar.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup falló" }

# 2) Instalador con la interfaz de Rust Sonar (installer\ui), que lleva el de Inno dentro.
$env:RUST_SONAR_PAYLOAD = "$root\dist\interno\$inner.exe"
cargo build --release --manifest-path installer\ui\Cargo.toml --target-dir target
if ($LASTEXITCODE -ne 0) { throw "no se pudo compilar el instalador (installer\ui)" }
$final = "dist\RustSonar-Instalador-$version-USO-PERSONAL.exe"
Copy-Item target\release\rust_sonar_setup.exe $final -Force
if ($cert -and $signtool) {
    & $signtool sign /sha1 $cert.Thumbprint /s My /fd SHA256 /d "Instalador de Rust Sonar" $final
    if ($LASTEXITCODE -ne 0) { throw "no se pudo firmar el instalador" }
}
# Restos de versiones anteriores del script (el instalador de Inno suelto en dist\).
Get-ChildItem dist\RustSonar-Setup-*.exe -ErrorAction SilentlyContinue | Remove-Item
Get-ChildItem dist\*.exe | Select-Object Name, @{n='MB'; e={[math]::Round($_.Length / 1MB, 1)}}
