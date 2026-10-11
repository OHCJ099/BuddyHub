param([Parameter(Mandatory=$true)][string]$Destination,[string]$Target='')
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
$argsList=@('metadata','--locked','--format-version','1','--manifest-path',(Join-Path $root 'native\Cargo.toml'))
if($Target){$argsList+=@('--filter-platform',$Target)}
$metadata=(& cargo @argsList | Out-String) | ConvertFrom-Json
if($LASTEXITCODE -ne 0){throw 'Could not read Rust dependencies'}
$inventory=@('# Rust dependency licenses','','Package | Version | SPDX expression','--- | --- | ---')
foreach($pkg in ($metadata.packages | Where-Object {$_.source} | Sort-Object name,version)){
    $inventory+=($pkg.name+' | '+$pkg.version+' | '+$pkg.license)
    $folder=Split-Path -Parent $pkg.manifest_path
    $files=Get-ChildItem -LiteralPath $folder -File | Where-Object {$_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|AUTHORS)([._-]|$)'}
    if($pkg.license_file){$files+=Get-Item -LiteralPath (Join-Path $folder $pkg.license_file)}
    $out=Join-Path $Destination ('rust\'+$pkg.name+'-'+$pkg.version)
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    $files | Sort-Object FullName -Unique | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination $out -Force}
}
$inventory | Set-Content -LiteralPath (Join-Path $Destination 'rust-dependencies.md') -Encoding UTF8
[xml]$project=Get-Content -LiteralPath (Join-Path $root 'desktop\BuddyHub.csproj') -Raw
$runtimeVersion=$project.Project.PropertyGroup.RuntimeFrameworkVersion | Select-Object -First 1
$nuget=if($env:NUGET_PACKAGES){$env:NUGET_PACKAGES}else{Join-Path $env:USERPROFILE '.nuget\packages'}
foreach($name in 'microsoft.netcore.app.runtime.win-x64','microsoft.windowsdesktop.app.runtime.win-x64'){
    $folder=Join-Path $nuget ($name+'\'+$runtimeVersion)
    $files=Get-ChildItem -LiteralPath $folder -File | Where-Object {$_.Name -match '^(LICENSE|THIRD-PARTY-NOTICES)(\.|$)'}
    if(!$files){throw ('Missing runtime license: '+$name)}
    $out=Join-Path $Destination $name
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    $files | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination $out}
}
