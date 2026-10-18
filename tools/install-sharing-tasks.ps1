param([bool]$AutoStart=$true)
$ErrorActionPreference='Stop'
$dataRoot=if($env:BUDDYHUB_DATA){$env:BUDDYHUB_DATA}else{Join-Path $env:USERPROFILE '.buddyhub'}
$shareRoot=Join-Path $dataRoot 'share'
if(!(Test-Path -LiteralPath (Join-Path $shareRoot 'ngrok.yml'))){throw 'ngrok configuration is not ready'}
$settings=New-ScheduledTaskSettingsSet -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
$trigger=New-ScheduledTaskTrigger -AtLogOn -User ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name)
$trigger.Enabled=$AutoStart
$principal=New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$gateway=New-ScheduledTaskAction -Execute (Join-Path $shareRoot 'buddyhub-share.exe')
$tunnel=New-ScheduledTaskAction -Execute (Join-Path $shareRoot 'buddyhub-share.exe') -Argument '--tunnel'
Register-ScheduledTask -TaskName 'BuddyHub Sharing Gateway' -Action $gateway -Trigger $trigger -Settings $settings -Principal $principal -Description 'Optional public API gateway with independent keys; local administration is excluded.' -Force | Out-Null
Register-ScheduledTask -TaskName 'BuddyHub Sharing Tunnel' -Action $tunnel -Trigger $trigger -Settings $settings -Principal $principal -Description 'Optional ngrok free tunnel for the sharing gateway; request inspection is disabled.' -Force | Out-Null
Start-ScheduledTask -TaskName 'BuddyHub Sharing Gateway'
Start-ScheduledTask -TaskName 'BuddyHub Sharing Tunnel'
Write-Output 'Sharing gateway and tunnel tasks installed and started.'
