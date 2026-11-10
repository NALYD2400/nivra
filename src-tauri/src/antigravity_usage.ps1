$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
try {
    # Associate quota data with a profile only after checking the reported identity.
    $servers = @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -match '^language_server.*\.exe$' -and $_.CommandLine -match '(?i)antigravity'
    } | Select-Object -First 8)
    if ($servers.Count -eq 0) { throw 'Aucun serveur Antigravity détecté.' }
    $listeners = @(& netstat.exe -ano -p tcp)
    $deadline = [System.Diagnostics.Stopwatch]::StartNew()
    $results = [System.Collections.Generic.List[object]]::new()
    $body = '{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"fr"}}'
    foreach ($server in $servers) {
        if ($deadline.Elapsed.TotalSeconds -ge 20) { break }
        if ($server.CommandLine -notmatch '--csrf_token(?:=|\s+)["'']?([^\s"'']+)') { continue }
        $csrf = $Matches[1]
        $ports = @($listeners | ForEach-Object {
            if ($_ -match '^\s*TCP\s+(?:127\.0\.0\.1|0\.0\.0\.0):(\d+)\s+\S+\s+LISTENING\s+(\d+)\s*$' -and [int]$Matches[2] -eq $server.ProcessId) { [int]$Matches[1] }
        } | Select-Object -Unique -First 12)
        $headers = @{ 'X-Codeium-Csrf-Token' = $csrf; 'Connect-Protocol-Version' = '1' }
        foreach ($port in $ports) {
            if ($deadline.Elapsed.TotalSeconds -ge 20) { break }
            try {
                $data = Invoke-RestMethod -Uri "http://127.0.0.1:$port/exa.language_server_pb.LanguageServerService/GetUserStatus" -Method Post -Headers $headers -ContentType 'application/json' -Body $body -TimeoutSec 2 -MaximumRedirection 0
                $status = $data.userStatus
                $email = [string]$status.email
                if (!$status -or !$email.Contains('@') -or $email.Length -gt 320) { continue }
                $models = @($status.cascadeModelConfigData.clientModelConfigs | Select-Object -First 30 | ForEach-Object {
                    $fraction = $_.quotaInfo.remainingFraction
                    if ($fraction -isnot [double] -and $fraction -isnot [single] -and $fraction -isnot [decimal] -and $fraction -isnot [int] -and $fraction -isnot [long]) { return }
                    $fraction = [double]$fraction
                    if ([double]::IsNaN($fraction) -or [double]::IsInfinity($fraction) -or $fraction -lt 0 -or $fraction -gt 1) { return }
                    $label = [string]$_.label
                    if ([string]::IsNullOrWhiteSpace($label) -or $label.Length -gt 300) { return }
                    @{ label = $label; remaining = $fraction; reset = [string]$_.quotaInfo.resetTime }
                })
                $windows = @()
                try {
                    $summary = Invoke-RestMethod -Uri "http://127.0.0.1:$port/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary" -Method Post -Headers $headers -ContentType 'application/json' -Body '{}' -TimeoutSec 2 -MaximumRedirection 0
                    $windows = @($summary.response.groups | Select-Object -First 30 | ForEach-Object {
                        $group = [string]$_.displayName
                        if ([string]::IsNullOrWhiteSpace($group) -or $group.Length -gt 150) { return }
                        @($_.buckets | Select-Object -First 50 | ForEach-Object {
                            $window = [string]$_.window
                            if ($window -notin @('5h', 'weekly')) { return }
                            $fraction = $_.remainingFraction
                            if ($fraction -isnot [double] -and $fraction -isnot [single] -and $fraction -isnot [decimal] -and $fraction -isnot [int] -and $fraction -isnot [long]) { return }
                            $fraction = [double]$fraction
                            if ([double]::IsNaN($fraction) -or [double]::IsInfinity($fraction) -or $fraction -lt 0 -or $fraction -gt 1) { return }
                            @{ group = $group; window = $window; remaining = $fraction; reset = [string]$_.resetTime }
                        })
                    })
                } catch { $windows = @() }
                $results.Add(@{ email = $email; plan = [string]$status.planStatus.planInfo.planName; models = $models; windows = $windows })
                break
            } catch { continue }
        }
    }
    if ($results.Count -eq 0) { throw 'Aucun quota associé à une identité vérifiable.' }
    ConvertTo-Json -InputObject @($results.ToArray()) -Depth 6 -Compress
} catch {
    [Console]::Error.Write("Lecture Antigravity indisponible. Vérifie qu’Antigravity est ouvert et connecté.")
    exit 1
}
