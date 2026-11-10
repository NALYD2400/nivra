param(
    [Parameter(Mandatory = $true)][ValidatePattern('^v\d+\.\d+\.\d+$')][string]$Tag,
    [string]$Repository = 'NALYD2400/nivra',
    [switch]$Publish,
    [switch]$RequirePublished
)
$ErrorActionPreference = 'Stop'
$version = $Tag.Substring(1)
$installer = "Nivra_${version}_x64-setup.exe"
$releaseText = & gh release view $Tag -R $Repository --json tagName,isDraft,isPrerelease,assets,url
if ($LASTEXITCODE -ne 0) { throw 'Impossible de lire la release.' }
$release = ConvertFrom-Json -InputObject ($releaseText -join "`n")
if ($release.tagName -ne $Tag -or $release.isPrerelease) { throw 'Version de release incorrecte.' }
foreach ($name in @($installer, "$installer.sig", 'latest.json')) {
    $asset = @($release.assets | Where-Object { $_.name -eq $name })
    if ($asset.Count -ne 1 -or $asset[0].state -ne 'uploaded' -or $asset[0].size -le 0) {
        throw "Artefact absent ou incomplet : $name"
    }
}
$workspace = Split-Path -Parent $PSScriptRoot
$checkRoot = Join-Path $workspace '.private/release-check'
$checkDirectory = Join-Path $checkRoot ([Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $checkDirectory -Force | Out-Null
& gh release download $Tag -R $Repository --pattern $installer --pattern "$installer.sig" --pattern latest.json --dir $checkDirectory
if ($LASTEXITCODE -ne 0) { throw 'Téléchargement des artefacts impossible.' }
$manifest = Get-Content -LiteralPath (Join-Path $checkDirectory 'latest.json') -Raw | ConvertFrom-Json
if ($manifest.version.TrimStart('v') -ne $version) { throw 'Le manifeste ne correspond pas à la version.' }
$signature = (Get-Content -LiteralPath (Join-Path $checkDirectory "$installer.sig") -Raw).Trim()
if ($signature.Length -lt 100) { throw 'Signature de mise à jour absente ou incomplète.' }
$platforms = @($manifest.platforms.PSObject.Properties | Where-Object { $_.Name -like 'windows-x86_64*' })
if ($platforms.Count -eq 0) { throw 'Plateforme Windows x64 absente du manifeste.' }
$expectedUrl = "https://github.com/$Repository/releases/download/$Tag/$installer"
$installerAsset = $release.assets | Where-Object { $_.name -eq $installer }
foreach ($platform in $platforms) {
    if ($platform.Value.url -notin @($expectedUrl, $installerAsset.apiUrl) -or $platform.Value.signature.Trim() -ne $signature) {
        throw "URL ou signature incorrecte pour $($platform.Name)."
    }
}
$installerPath = Join-Path $checkDirectory $installer
$sha256 = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ((Get-Item -LiteralPath $installerPath).Length -ne $installerAsset.size) { throw 'Taille de l’installateur incorrecte.' }
if ($installerAsset.digest -ne "sha256:$sha256") { throw 'Le SHA-256 téléchargé diffère de celui de GitHub.' }
if ($Publish) {
    if (-not $release.isDraft) { throw 'La publication doit partir d’une release brouillon vérifiée.' }
    & gh release edit $Tag -R $Repository --draft=false --latest
    if ($LASTEXITCODE -ne 0) { throw 'La publication de la release a échoué.' }
    $publishedText = & gh release view $Tag -R $Repository --json isDraft,isPrerelease
    if ($LASTEXITCODE -ne 0) { throw 'Impossible de vérifier la publication.' }
    $published = ConvertFrom-Json -InputObject ($publishedText -join "`n")
    if ($published.isDraft -or $published.isPrerelease) { throw 'La release n’est pas publique et stable.' }
} elseif ($RequirePublished -and $release.isDraft) { throw 'La release est encore en brouillon.' }
Write-Output "Release vérifiée : $($release.url)"
Write-Output "Installateur : $installer"
Write-Output "SHA-256 : $sha256"
Write-Output "Manifeste et signature vérifiés pour $($platforms.Count) plateforme(s) Windows x64."
