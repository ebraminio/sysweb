param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [int]$Port = 19222
)

$ErrorActionPreference = "Stop"
$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$root = Join-Path $base "sysweb-win-root"
$outside = Join-Path $base "sysweb-win-outside"
Remove-Item -Recurse -Force $root, $outside -ErrorAction SilentlyContinue
New-Item -ItemType Directory $root, $outside | Out-Null
Set-Content -NoNewline -Path (Join-Path $root "hello.txt") -Value "windows smoke"
Set-Content -NoNewline -Path (Join-Path $outside "secret.txt") -Value "outside secret"
cmd /c mklink /J (Join-Path $root "link") $outside | Out-Null

$proc = Start-Process -FilePath $Exe `
    -ArgumentList "--root", $root, "--port", $Port, "--upload", "--quiet" -PassThru
Start-Sleep -Milliseconds 800
try {
    $response = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$Port/hello.txt"
    if ($response.StatusCode -ne 200 -or $response.Content -ne "windows smoke") {
        throw "GET failed"
    }

    $put = Invoke-WebRequest -UseBasicParsing -Method Put -Body "put smoke" `
        "http://127.0.0.1:$Port/put.txt"
    if ($put.StatusCode -ne 201) { throw "PUT failed" }

    $form = Join-Path $root "put.txt"
    $post = & curl.exe -sS -o NUL -w "%{http_code}" -F "file=@$form;filename=post.txt" `
        "http://127.0.0.1:$Port/"
    if ($post -ne "201") { throw "POST failed: $post" }

    $range = & curl.exe -sS -H "Range: bytes=1-3" "http://127.0.0.1:$Port/hello.txt"
    if ($range -ne "ind") { throw "range failed: $range" }

    # A junction inside the root must not expose the outside directory.
    try {
        $leak = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$Port/link/secret.txt"
        if ($leak.StatusCode -eq 200) { throw "junction escaped the root" }
    } catch {
        $code = $_.Exception.Response.StatusCode.value__
        if ($code -ne 403) {
            throw "junction returned $code, expected 403"
        }
    }

    $jobs = 1..3 | ForEach-Object {
        Start-Job -ScriptBlock {
            param($url)
            (Invoke-WebRequest -UseBasicParsing $url).StatusCode
        } -ArgumentList "http://127.0.0.1:$Port/"
    }
    $codes = $jobs | Wait-Job | Receive-Job
    $jobs | Remove-Job
    if (($codes | Where-Object { $_ -ne 200 }).Count -ne 0) {
        throw "concurrent GET failed"
    }

    for ($i = 0; $i -lt 50; $i++) {
        $load = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$Port/"
        if ($load.StatusCode -ne 200) { throw "sequential load failed" }
    }

    # Idle deadline: a partial request must be closed.
    $idlePort = $Port + 1
    $idleProc = Start-Process -FilePath $Exe `
        -ArgumentList "--root", $root, "--port", $idlePort, "--idle-timeout", "1", "--quiet" -PassThru
    Start-Sleep -Milliseconds 700
    try {
        $client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $idlePort)
        $stream = $client.GetStream()
        $bytes = [Text.Encoding]::ASCII.GetBytes("GET / HTTP/1.1`r`nHost: x`r`n")
        $stream.Write($bytes, 0, $bytes.Length)
        Start-Sleep -Milliseconds 1500
        $stream.ReadTimeout = 1000
        $buffer = New-Object byte[] 1
        $read = $stream.Read($buffer, 0, 1)
        $client.Close()
        if ($read -ne 0) { throw "idle connection was not closed" }
    } finally {
        Stop-Process -Id $idleProc.Id -Force -ErrorAction SilentlyContinue
    }

    # Blocking fallback still serves.
    $blockPort = $Port + 2
    $blockProc = Start-Process -FilePath $Exe `
        -ArgumentList "--root", $root, "--port", $blockPort, "--blocking", "--quiet" -PassThru
    Start-Sleep -Milliseconds 700
    try {
        $blocked = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$blockPort/hello.txt"
        if ($blocked.StatusCode -ne 200) { throw "blocking fallback failed" }
    } finally {
        Stop-Process -Id $blockProc.Id -Force -ErrorAction SilentlyContinue
    }

    Write-Output "windows smoke: GET/PUT/POST/range/junction/concurrency/idle/blocking OK"
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $root, $outside -ErrorAction SilentlyContinue
}
