param([string]$Target='')
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$cargoArgs=@('--manifest-path',(Join-Path $root 'native\Cargo.toml'),'--locked')
if($Target){$cargoArgs+=@('--target',$Target)}
& cargo test @cargoArgs
if($LASTEXITCODE -ne 0){throw 'Rust tests failed'}
& cargo build --release @cargoArgs
if($LASTEXITCODE -ne 0){throw 'Rust build failed'}
$targetRoot=if($env:CARGO_TARGET_DIR){$env:CARGO_TARGET_DIR}else{Join-Path $root 'native\target'}
if($Target){$targetRoot=Join-Path $targetRoot $Target}
$dist=Join-Path $root 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
foreach($name in 'buddyhub-service.exe','buddyhub-share.exe'){
    Copy-Item -LiteralPath (Join-Path $targetRoot ('release\'+$name)) -Destination $dist -Force
}
