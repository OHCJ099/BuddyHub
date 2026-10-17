param([string]$Version='1.0.0',[string]$NativeTarget='')
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$dist=Join-Path $root 'dist'
& (Join-Path $PSScriptRoot 'build-native.ps1') -Target $NativeTarget
& dotnet publish (Join-Path $root 'desktop\BuddyHub.csproj') -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -p:DebugType=None -p:Version=$Version -o $dist --nologo
if($LASTEXITCODE -ne 0){throw '.NET publish failed'}
$package=Join-Path $root ('artifacts\BuddyHub-v'+$Version+'-windows-x64')
if(Test-Path -LiteralPath $package){throw 'Package directory already exists; use a fresh artifacts directory'}
New-Item -ItemType Directory -Path (Join-Path $package 'tools') -Force | Out-Null
foreach($name in 'BuddyHub.exe','buddyhub-service.exe'){
    Copy-Item -LiteralPath (Join-Path $dist $name) -Destination $package
}
foreach($name in 'install.ps1','uninstall.ps1'){
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $package 'tools')
}
foreach($name in 'README.md','LICENSE','THIRD_PARTY_NOTICES.md','setup.cmd'){
    Copy-Item -LiteralPath (Join-Path $root $name) -Destination $package
}
$sourceCommit='unknown'
if((Get-Command git -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath (Join-Path $root '.git'))){
    $sourceCommit=(& git -C $root rev-parse HEAD | Out-String).Trim()
    if($LASTEXITCODE -ne 0){throw 'Could not determine source commit'}
}
@{version=$Version;sourceCommit=$sourceCommit;builtAtUtc=[DateTime]::UtcNow.ToString('o')} |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $package 'BUILD_INFO.json') -Encoding UTF8
& (Join-Path $PSScriptRoot 'collect-licenses.ps1') -Destination (Join-Path $package 'licenses') -Target $NativeTarget
$zip=$package+'.zip'
Compress-Archive -LiteralPath $package -DestinationPath $zip
$hash=Get-FileHash -LiteralPath $zip -Algorithm SHA256
($hash.Hash.ToLowerInvariant()+'  '+[IO.Path]::GetFileName($zip)) | Set-Content -LiteralPath ($zip+'.sha256') -Encoding ASCII
Write-Output $zip
