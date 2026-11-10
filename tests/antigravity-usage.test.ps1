$ErrorActionPreference = 'Stop'
function Get-CimInstance {
    param($ClassName)
    @(
        [pscustomobject]@{ Name='language_server_windows.exe'; CommandLine='--antigravity --csrf_token=test-alice'; ProcessId=101 },
        [pscustomobject]@{ Name='language_server_windows.exe'; CommandLine='--antigravity --csrf_token=test-bob'; ProcessId=202 },
        [pscustomobject]@{ Name='language_server_windows.exe'; CommandLine='--antigravity --csrf_token=test-invalid'; ProcessId=303 }
    )
}
function netstat.exe {
    @(
        ' TCP    127.0.0.1:9001    0.0.0.0:0    LISTENING    101',
        ' TCP    127.0.0.1:9002    0.0.0.0:0    LISTENING    202',
        ' TCP    127.0.0.1:9003    0.0.0.0:0    LISTENING    303',
        ' TCP    192.168.1.1:9999  0.0.0.0:0    LISTENING    101'
    )
}
function Invoke-RestMethod {
    param($Uri, $Method, $Headers, $ContentType, $Body, $TimeoutSec, $MaximumRedirection)
    if ($MaximumRedirection -ne 0) { throw 'Redirects must be disabled' }
    $index = if ($Uri -match ':9001/') { 1 } elseif ($Uri -match ':9002/') { 2 } elseif ($Uri -match ':9003/') { 3 } else { throw 'Unexpected endpoint' }
    $token = @('unused','test-alice','test-bob','test-invalid')[$index]
    if ($Headers['X-Codeium-Csrf-Token'] -ne $token) { throw 'CSRF token belongs to another process' }
    if ($Uri -match '/RetrieveUserQuotaSummary$') {
        if ($index -eq 2) { throw 'Summary is unavailable' }
        return @{ response=@{ groups=@(@{displayName='Gemini Models';buckets=@(
            @{window='5h';remainingFraction=0.25;resetTime='2030-01-01T00:00:00Z'},
            @{window='weekly';remainingFraction=0;resetTime=''},
            @{window='weekly';remainingFraction='unknown'},
            @{window='unexpected';remainingFraction=0.5}
        )})} }
    }
    $email = @('unused','alice@example.com','bob@example.com','unknown')[$index]
    return @{ userStatus=@{ email=$email; planStatus=@{planInfo=@{planName='Pro'}}; cascadeModelConfigData=@{clientModelConfigs=@(
        @{label="Model $index";quotaInfo=@{remainingFraction=0.5;resetTime='2030-01-01T00:00:00Z'}},
        @{label='Malformed';quotaInfo=@{remainingFraction='not-a-number'}},
        @{label='Outside range';quotaInfo=@{remainingFraction=5}},
        @{label='Empty';quotaInfo=@{remainingFraction=$null}},
        @{label='Second valid model';quotaInfo=@{remainingFraction=0;resetTime=''}}
    )} } }
}
$scriptPath = Join-Path $PSScriptRoot '../src-tauri/src/antigravity_usage.ps1'
$json = & $scriptPath
$result = ConvertFrom-Json -InputObject $json
if ($result.Count -ne 2) { throw "Expected two verified accounts; got $($result.Count)" }
if ($result[0].email -ne 'alice@example.com' -or $result[0].models[0].label -ne 'Model 1') { throw 'Alice quota was not matched to Alice' }
if ($result[1].email -ne 'bob@example.com' -or $result[1].models[0].label -ne 'Model 2') { throw 'Bob quota was not matched to Bob' }
if ($result[0].models.Count -ne 2 -or $result[0].models[1].label -ne 'Second valid model' -or $result[0].models[1].remaining -ne 0) { throw 'Invalid model quotas hid valid models or produced a fabricated value' }
if ($result[0].windows.Count -ne 2 -or $result[0].windows[0].remaining -ne 0.25 -or $result[0].windows[1].remaining -ne 0) { throw 'Quota windows were not preserved correctly' }
if ($result[1].windows.Count -ne 0) { throw 'Unavailable summary should not invent quota windows' }
if ($json -match 'test-alice|test-bob') { throw 'Secret unexpectedly included in output' }
Write-Output 'Antigravity multi-session parsing passed.'
