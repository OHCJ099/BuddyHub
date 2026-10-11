$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
$exe=Join-Path $root 'BuddyHub.exe'
$service=Join-Path $root 'buddyhub-service.exe'
foreach($name in 'BuddyHub Background','BuddyHub Daily Checkin','BuddyHub Startup Guard'){
    $task=Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if($task -and $task.Actions.Execute -contains $service){
        Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $name -Confirm:$false
    }
}
$run='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if((Get-ItemProperty -LiteralPath $run -Name BuddyHubBackground -ErrorAction SilentlyContinue).BuddyHubBackground -eq ('"'+$service+'" --daemon')){
    Remove-ItemProperty -LiteralPath $run -Name BuddyHubBackground
}
foreach($p in (Get-Process BuddyHub,buddyhub-service -ErrorAction SilentlyContinue)){
    if($p.Path -eq $exe -or $p.Path -eq $service){Stop-Process -Id $p.Id -Force}
}
Write-Output 'Autostart removed. Account data and files are preserved.'
