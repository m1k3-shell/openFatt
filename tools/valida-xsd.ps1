# Valida file FatturaPA contro lo schema XSD ufficiale (VFPR12 1.2.3) con il validatore .NET.
# Uso: powershell -File tools\valida-xsd.ps1 [cartella-o-file ...]
param([string[]]$Percorsi = @("target\xsd-samples"))
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$xsdDir = Join-Path $root 'specs\xsd'

$settings = [Xml.XmlReaderSettings]::new()
$settings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
$settings.ValidationType = [Xml.ValidationType]::Schema
# lo schema importa xmldsig da un URL: si usa la copia locale
$dsig = [Xml.XmlReader]::Create((Join-Path $xsdDir 'xmldsig-core-schema.xsd'), (New-Object Xml.XmlReaderSettings -Property @{ DtdProcessing = [Xml.DtdProcessing]::Ignore }))
[void]$settings.Schemas.Add('http://www.w3.org/2000/09/xmldsig#', $dsig)
$fpr = Get-Content -Raw -Encoding UTF8 (Join-Path $xsdDir 'Schema_VFPR12_v1.2.3.xsd')
$fpr = $fpr -replace 'schemaLocation="http://www.w3.org/TR/2002/REC-xmldsig-core-20020212/xmldsig-core-schema.xsd"', ''
[void]$settings.Schemas.Add('http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v1.2', [Xml.XmlReader]::Create([IO.StringReader]::new($fpr)))
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
