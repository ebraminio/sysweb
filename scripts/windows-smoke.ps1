param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [int]$Port = 19222
)

$ErrorActionPreference = "Stop"

function Read-HttpResponse($stream) {
    $bytes = New-Object System.Collections.Generic.List[byte]
    $one = New-Object byte[] 1
    while ($true) {
        $n = $stream.Read($one, 0, 1)
        if ($n -le 0) { throw "unexpected EOF in HTTP header" }
        $bytes.Add($one[0])
        $count = $bytes.Count
        if ($count -ge 4 -and
            $bytes[$count - 4] -eq 13 -and $bytes[$count - 3] -eq 10 -and
            $bytes[$count - 2] -eq 13 -and $bytes[$count - 1] -eq 10) {
            break
        }
    }
    $head = [Text.Encoding]::ASCII.GetString($bytes.ToArray())
    if ($head -notmatch '(?im)^Content-Length:\s*(\d+)') {
        throw "missing Content-Length"
    }
    $length = [int]$Matches[1]
    $body = New-Object byte[] $length
    $offset = 0
    while ($offset -lt $length) {
        $n = $stream.Read($body, $offset, $length - $offset)
        if ($n -le 0) { throw "unexpected EOF in HTTP body" }
        $offset += $n
    }
    [pscustomobject]@{ Head = $head; Body = $body }
}
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

    $ka = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $Port)
    $kaStream = $ka.GetStream()
    $kaStream.ReadTimeout = 5000
    $first = [Text.Encoding]::ASCII.GetBytes("GET /hello.txt HTTP/1.1`r`nHost: x`r`n`r`n")
    $kaStream.Write($first, 0, $first.Length)
    $r1 = Read-HttpResponse $kaStream
    if ($r1.Head -notmatch 'Connection: keep-alive') { throw "keep-alive response header missing" }
    $second = [Text.Encoding]::ASCII.GetBytes("GET /hello.txt HTTP/1.1`r`nHost: x`r`nConnection: close`r`n`r`n")
    $kaStream.Write($second, 0, $second.Length)
    $r2 = Read-HttpResponse $kaStream
    if ($r2.Head -notmatch 'Connection: close') { throw "close response header missing" }
    $ka.Close()

    # A second instance must not be able to bind the same port.
    $second = Start-Process -FilePath $Exe `
        -ArgumentList "--root", $root, "--port", $Port, "--quiet" -PassThru `
        -RedirectStandardError (Join-Path $base "sysweb-second.err")
    if (-not $second.WaitForExit(2000)) {
        Stop-Process -Id $second.Id -Force
        throw "second instance bound the same port"
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

    try {
        Invoke-WebRequest -UseBasicParsing -Method Put -Body "escape" `
            "http://127.0.0.1:$Port/link/escaped.txt" | Out-Null
        throw "junction write escaped the root"
    } catch {
        $code = $_.Exception.Response.StatusCode.value__
        if ($code -ne 403) { throw "junction write returned $code, expected 403" }
        if (Test-Path (Join-Path $outside "escaped.txt")) {
            throw "junction write created an outside file"
        }
    }

    $jobs = 1..4 | ForEach-Object {
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

    # A stalled large download must not prevent another request.
    $large = New-Object byte[] (2 * 1024 * 1024)
    [IO.File]::WriteAllBytes((Join-Path $root "large.bin"), $large)
    $slow = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $Port)
    $slowStream = $slow.GetStream()
    $request = [Text.Encoding]::ASCII.GetBytes("GET /large.bin HTTP/1.1`r`nHost: x`r`n`r`n")
    $slowStream.Write($request, 0, $request.Length)
    Start-Sleep -Milliseconds 100
    $live = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$Port/hello.txt"
    if ($live.StatusCode -ne 200) { throw "stalled download blocked the server" }
    $slow.Close()

    # Idle deadline: a partial request must be closed.
    $idlePort = $Port + 1
    $idleProc = Start-Process -FilePath $Exe `
        -ArgumentList "--root", $root, "--port", $idlePort, "--idle-timeout", "1", "--quiet" -PassThru
    Start-Sleep -Milliseconds 700
    try {
        $clients = @()
        $bytes = [Text.Encoding]::ASCII.GetBytes("GET / HTTP/1.1`r`nHost: x`r`n")
        for ($i = 0; $i -lt 4; $i++) {
            $client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $idlePort)
            $client.GetStream().Write($bytes, 0, $bytes.Length)
            $clients += $client
        }
        Start-Sleep -Milliseconds 1500
        foreach ($client in $clients) {
            $client.GetStream().ReadTimeout = 1000
            $buffer = New-Object byte[] 1
            $read = $client.GetStream().Read($buffer, 0, 1)
            $client.Close()
            if ($read -ne 0) { throw "idle connection was not closed" }
        }
        $recovered = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$idlePort/hello.txt"
        if ($recovered.StatusCode -ne 200) { throw "server did not recover after idle timeout" }
    } finally {
        Stop-Process -Id $idleProc.Id -Force -ErrorAction SilentlyContinue
    }

    # Blocking fallback still serves.
    $blockPort = $Port + 2
    $blockProc = Start-Process -FilePath $Exe `
        -ArgumentList "--root", $root, "--port", $blockPort, "--blocking", "--quiet" -PassThru
    $blocked = $null
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        Start-Sleep -Milliseconds 100
        try {
            $blocked = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$blockPort/hello.txt"
            break
        } catch {}
    }
    try {
        if (-not $blocked -or $blocked.StatusCode -ne 200) { throw "blocking fallback failed" }
    } finally {
        Stop-Process -Id $blockProc.Id -Force -ErrorAction SilentlyContinue
    }

    Write-Output "windows smoke: GET/PUT/POST/range/junction/concurrency/idle/blocking OK"
} finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $root, $outside -ErrorAction SilentlyContinue
}
