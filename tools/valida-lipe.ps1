# Valida i file della Comunicazione liquidazioni periodiche IVA contro lo schema ufficiale
# (fornituraIvp_2018_v1.xsd, in specs\lipe\xsd) con il validatore .NET.
# Uso: powershell -File tools\valida-lipe.ps1 [cartella-o-file ...]
param([string[]]$Percorsi = @("target\xsd-samples\lipe"))
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$xsd = Join-Path $root 'specs\lipe\xsd\ivp\2018\fornituraIvp_2018_v1.xsd'

$settings = [Xml.XmlReaderSettings]::new()
$settings.DtdProcessing = [Xml.DtdProcessing]::Parse
$settings.ValidationType = [Xml.ValidationType]::Schema
$schemaSettings = [Xml.XmlReaderSettings]::new()
$schemaSettings.DtdProcessing = [Xml.DtdProcessing]::Ignore
$resolver = [Xml.XmlUrlResolver]::new()
$settings.XmlResolver = $resolver
$settings.Schemas.XmlResolver = $resolver
[void]$settings.Schemas.Add('urn:www.agenziaentrate.gov.it:specificheTecniche:sco:ivp', [Xml.XmlReader]::Create($xsd, $schemaSettings))
$settings.Schemas.Compile()

$files = foreach ($p in $Percorsi) {
    $full = if ([IO.Path]::IsPathRooted($p)) { $p } else { Join-Path $root $p }
    if (Test-Path $full -PathType Container) { Get-ChildItem $full -Filter *.xml } else { Get-Item $full }
}
$bad = 0
foreach ($f in $files) {
    $issues = [Collections.Generic.List[string]]::new()
    $s = $settings.Clone()
    $s.add_ValidationEventHandler({ param($x, $e) $issues.Add("riga $($e.Exception.LineNumber): $($e.Message)") })
    $r = [Xml.XmlReader]::Create($f.FullName, $s)
    try { while ($r.Read()) {} } catch { $issues.Add($_.Exception.Message) } finally { $r.Dispose() }
    if ($issues.Count -eq 0) { Write-Output "VALIDO   $($f.Name)" } else { $bad++; Write-Output "INVALIDO $($f.Name)"; $issues | ForEach-Object { Write-Output "         $_" } }
}
if ($bad -gt 0) { exit 1 }
