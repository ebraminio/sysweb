param(
    [Parameter(Mandatory = $true)][string]$Exe
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $Exe)) { throw "missing binary: $Exe" }

$dumpbin = (Get-Command dumpbin.exe -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty Source)
if (-not $dumpbin) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $dumpbin = & $vswhere -latest -find "VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe" |
            Select-Object -First 1
    }
}
if (-not $dumpbin) { throw "dumpbin.exe not found; run from a VS developer shell" }

$dependents = & $dumpbin /nologo /dependents $Exe | Out-String
$imports = & $dumpbin /nologo /imports $Exe | Out-String
Write-Output $dependents

$forbidden = "vcruntime|msvcrt|ucrtbase|api-ms-win-crt"
if ("$dependents`n$imports" -match $forbidden) {
    throw "CRT/runtime dependency found"
}

$dllPattern = '(?im)^\s*([A-Za-z0-9_.-]+\.dll)\s*$'
$expected = @("KERNEL32.DLL", "SHELL32.DLL", "WS2_32.DLL")
$found = @()
foreach ($match in [regex]::Matches($dependents, $dllPattern)) {
    $found += $match.Groups[1].Value.ToUpperInvariant()
}
$found = $found | Sort-Object -Unique
$difference = Compare-Object -ReferenceObject $expected -DifferenceObject $found
if ($difference) {
    $text = ($difference | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }) -join ", "
    throw "unexpected DLL set: $text"
}

Write-Output ("windows imports: " + ($found -join ", "))
