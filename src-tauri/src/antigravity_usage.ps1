$ErrorActionPreference = 'Stop'
try {
    $servers = @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -match '^language_server.*\.exe$' -and $_.CommandLine -match '(?i)antigravity'
    })
    if ($servers.Count -ne 1) { throw 'Serveur Antigravity indisponible ou plusieurs sessions ouvertes.' }
    $server = $servers[0]
    if ($server.CommandLine -notmatch '--csrf_token(?:=|\s+)["'']?([^\s"'']+)') { throw 'Serveur Antigravity indisponible.' }
    $csrf = $Matches[1]
    $ports = @(& netstat.exe -ano -p tcp | ForEach-Object {
        if ($_ -match '^\s*TCP\s+(?:127\.0\.0\.1|0\.0\.0\.0):(\d+)\s+\S+\s+LISTENING\s+(\d+)\s*$' -and [int]$Matches[2] -eq $server.ProcessId) { [int]$Matches[1] }
    } | Select-Object -Unique -First 12)
    $body = '{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"fr"}}'
    $headers = @{ 'X-Codeium-Csrf-Token' = $csrf; 'Connect-Protocol-Version' = '1' }
    foreach ($port in $ports) {
        try {
            $data = Invoke-RestMethod -Uri "http://127.0.0.1:$port/exa.language_server_pb.LanguageServerService/GetUserStatus" -Method Post -Headers $headers -ContentType 'application/json' -Body $body -TimeoutSec 2
            $status = $data.userStatus
            if (!$status) { continue }
            $models = @($status.cascadeModelConfigData.clientModelConfigs | Where-Object { $_.quotaInfo -and $null -ne $_.quotaInfo.remainingFraction } | Select-Object -First 30 | ForEach-Object {
                @{ label = [string]$_.label; remaining = [double]$_.quotaInfo.remainingFraction; reset = [string]$_.quotaInfo.resetTime }
            })
            $result = @{ email = [string]$status.email; plan = [string]$status.planStatus.planInfo.planName; models = $models }
            $result | ConvertTo-Json -Depth 6 -Compress
            exit 0
        } catch { continue }
    }
    throw 'Quotas indisponibles. Vérifie qu’Antigravity est ouvert et connecté.'
} catch {
    [Console]::Error.Write('Lecture Antigravity indisponible. Vérifie qu’Antigravity est ouvert et connecté.')
    exit 1
}
