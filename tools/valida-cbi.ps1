# Valida i file XML per la banca (bonifici CBIPaymentRequest.00.04.01 e addebiti
# CBISDDReqLogMsg.00.01.01) contro gli schemi CBI in specs\cbi, con il validatore .NET.
# Uso: powershell -File tools\valida-cbi.ps1 [cartella-o-file ...]
param([string[]]$Percorsi = @("target\xsd-samples\cbi"))
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$xsdDir = Join-Path $root 'specs\cbi'

$settings = [Xml.XmlReaderSettings]::new()
$settings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
$settings.ValidationType = [Xml.ValidationType]::Schema
[void]$settings.Schemas.Add('urn:CBI:xsd:CBIPaymentRequest.00.04.01', (Join-Path $xsdDir 'CBIPaymentRequest.00.04.01.xsd'))
[void]$settings.Schemas.Add('urn:CBI:xsd:CBISDDReqLogMsg.00.01.01', (Join-Path $xsdDir 'CBISDDReqLogMsg.00.01.01.xsd'))
$settings.Schemas.Compile()

$files = foreach ($p in $Percorsi) {
    $full = if ([IO.Path]::IsPathRooted($p)) { $p } else { Join-Path $root $p }
    if (Test-Path $full -PathType Container) { Get-ChildItem $full -Filter *.xml } else { Get-Item $full }
}
$bad = 0
foreach ($f in $files) {
    $issues = [Collections.Generic.List[string]]::new()
    $s = $settings.Clone()
    $s.ValidationFlags = $s.ValidationFlags -bor [Xml.Schema.XmlSchemaValidationFlags]::ReportValidationWarnings
    $s.add_ValidationEventHandler({ param($x, $e) $issues.Add("riga $($e.Exception.LineNumber): $($e.Message)") })
    $r = [Xml.XmlReader]::Create($f.FullName, $s)
    try { while ($r.Read()) {} } catch { $issues.Add($_.Exception.Message) } finally { $r.Dispose() }
    if ($issues.Count -eq 0) { Write-Output "VALIDO   $($f.Name)" } else { $bad++; Write-Output "INVALIDO $($f.Name)"; $issues | ForEach-Object { Write-Output "         $_" } }
}
if ($bad -gt 0) { exit 1 }
