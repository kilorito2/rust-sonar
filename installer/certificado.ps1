# Crea (si no existe) el certificado de firma de código "kilorito" y lo marca como de confianza en
# este PC, para que Windows muestre "Editor comprobado: kilorito" en los avisos de administrador de
# Rust Sonar y de su instalador.
#
# - La clave privada se guarda en el almacén del usuario actual y NO es exportable.
# - El certificado solo vale para firmar código (no para webs ni correo).
# - Es autofirmado: solo este PC confía en él. En otros PCs seguirá saliendo "Editor desconocido".
#
# Uso: powershell -ExecutionPolicy Bypass -File installer\certificado.ps1
$ErrorActionPreference = 'Stop'
$subject = 'CN=kilorito'

$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
    Where-Object { $_.Subject -eq $subject -and $_.NotAfter -gt (Get-Date) } |
    Sort-Object NotAfter -Descending | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -FriendlyName 'kilorito (firma de Rust Sonar)' `
        -KeyExportPolicy NonExportable -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 `
        -NotAfter (Get-Date).AddYears(10) -CertStoreLocation Cert:\CurrentUser\My
    Write-Host "Certificado creado: $($cert.Thumbprint)"
} else {
    Write-Host "Certificado existente: $($cert.Thumbprint)"
}

# Parte pública, para confiar en ella (y por si se quiere instalar en otro PC propio).
$cer = Join-Path $PSScriptRoot 'kilorito.cer'
Export-Certificate -Cert $cert -FilePath $cer | Out-Null

$trusted = (Test-Path "Cert:\LocalMachine\Root\$($cert.Thumbprint)") -and (Test-Path "Cert:\LocalMachine\TrustedPublisher\$($cert.Thumbprint)")
if (-not $trusted) {
    Write-Host 'Marcándolo como de confianza en este PC (pide permisos de administrador)...'
    $cmd = "Import-Certificate -FilePath '$cer' -CertStoreLocation Cert:\LocalMachine\Root | Out-Null; " +
           "Import-Certificate -FilePath '$cer' -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null"
    Start-Process powershell.exe -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile', '-Command', $cmd
}
$ok = (Test-Path "Cert:\LocalMachine\Root\$($cert.Thumbprint)") -and (Test-Path "Cert:\LocalMachine\TrustedPublisher\$($cert.Thumbprint)")
Write-Host "De confianza en este PC: $ok"
