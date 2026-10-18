$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$releaseDir=if(Test-Path -LiteralPath (Join-Path $root 'BuddyHub.exe')){$root}else{Join-Path $root 'dist'}
$exe=Join-Path $releaseDir 'BuddyHub.exe'
$service=Join-Path $releaseDir 'buddyhub-service.exe'
foreach($name in 'BuddyHub Background','BuddyHub Daily Checkin','BuddyHub Startup Guard'){
    $task=Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if($task -and $task.Actions.Execute -contains $service){
        Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $name -Confirm:$false
    }
}
$dataDir=if($env:BUDDYHUB_DATA){$env:BUDDYHUB_DATA}else{Join-Path $env:USERPROFILE '.buddyhub'}
$shareRoot=Join-Path $dataDir 'share'
$shareBinary=Join-Path $shareRoot 'buddyhub-share.exe'
foreach($name in 'BuddyHub Sharing Gateway','BuddyHub Sharing Tunnel'){
    $task=Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if($task -and $task.Actions.Execute -contains $shareBinary){
        Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $name -Confirm:$false
    }
}
Get-Process buddyhub-share,ngrok -ErrorAction SilentlyContinue | Where-Object {$_.Path -and [IO.Path]::GetDirectoryName($_.Path) -eq $shareRoot} | Stop-Process -Force
$run='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if((Get-ItemProperty -LiteralPath $run -Name BuddyHubBackground -ErrorAction SilentlyContinue).BuddyHubBackground -eq ('"'+$service+'" --daemon')){
    Remove-ItemProperty -LiteralPath $run -Name BuddyHubBackground
}
foreach($p in (Get-Process BuddyHub,buddyhub-service -ErrorAction SilentlyContinue)){
    if($p.Path -eq $exe -or $p.Path -eq $service){Stop-Process -Id $p.Id -Force}
}
Write-Output 'Autostart removed. Account data and files are preserved.'
