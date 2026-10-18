$ErrorActionPreference = 'Stop'
$appRoot = Split-Path -Parent $PSScriptRoot
$releaseDir = if(Test-Path -LiteralPath (Join-Path $appRoot 'BuddyHub.exe')){$appRoot}else{Join-Path $appRoot 'dist'}
$exePath = Join-Path $releaseDir 'BuddyHub.exe'
$servicePath = Join-Path $releaseDir 'buddyhub-service.exe'
if (!(Test-Path -LiteralPath $exePath)) { throw '请先编译 BuddyHub' }
if (!(Test-Path -LiteralPath $servicePath)) { throw '请先编译原生后台' }
$dataDir = if($env:BUDDYHUB_DATA){$env:BUDDYHUB_DATA}else{Join-Path $env:USERPROFILE '.buddyhub'}
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
$sharingBinary=Join-Path $releaseDir 'buddyhub-share.exe'
$sharingTarget=Join-Path $dataDir 'share\buddyhub-share.exe'
if(Test-Path -LiteralPath $sharingBinary){
    New-Item -ItemType Directory -Path (Split-Path -Parent $sharingTarget) -Force | Out-Null
    $running=Get-Process buddyhub-share -ErrorAction SilentlyContinue | Where-Object {$_.Path -eq $sharingTarget}
    if(!$running){Copy-Item -LiteralPath $sharingBinary -Destination $sharingTarget -Force}
    else{Write-Warning '共享后台正在运行；更新该组件前，请先在旧窗口关闭公网服务，再运行安装。'}
}
$aclUser = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
& icacls.exe $dataDir '/inheritance:r' '/grant:r' ($aclUser + ':(OI)(CI)F') 'SYSTEM:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw '无法限制数据目录权限' }

& $servicePath --init
if($LASTEXITCODE -ne 0){throw '首次初始化失败，请查看数据目录中的 native-errors.log'}

$runPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$taskUser = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$principal = New-ScheduledTaskPrincipal -UserId $taskUser -LogonType Interactive -RunLevel Limited
$backgroundSettings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
$backgroundAction = New-ScheduledTaskAction -Execute $servicePath -Argument '--daemon' -WorkingDirectory $releaseDir
$logonTrigger = New-ScheduledTaskTrigger -AtLogOn -User ([System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
$recoveryTrigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName 'BuddyHub Background' -Action $backgroundAction -Trigger @($logonTrigger,$recoveryTrigger) -Principal $principal -Settings $backgroundSettings -Description '登录后静默启动反代与签到后台；每分钟补偿检查；失败自动重试；不打开管理窗口' -Force | Out-Null
# Windows Run and Task Scheduler provide independent logon paths. The daemon
# binds its management port before starting work, so only one instance serves.
New-ItemProperty -LiteralPath $runPath -Name BuddyHubBackground -PropertyType String -Value ('"' + $servicePath + '" --daemon') -Force | Out-Null
$taskSettings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 5)
$checkinAction = New-ScheduledTaskAction -Execute $servicePath -Argument '--checkin' -WorkingDirectory $releaseDir
$dailyTrigger = New-ScheduledTaskTrigger -Daily -At '09:00'
Register-ScheduledTask -TaskName 'BuddyHub Daily Checkin' -Action $checkinAction -Trigger $dailyTrigger -Principal $principal -Settings $taskSettings -Description '中国版账号每日签到；后台启动后补签当天' -Force | Out-Null
$guardAction = New-ScheduledTaskAction -Execute $servicePath -Argument '--guard' -WorkingDirectory $releaseDir
$guardTrigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5) -RepetitionInterval (New-TimeSpan -Minutes 5)
Register-ScheduledTask -TaskName 'BuddyHub Startup Guard' -Action $guardAction -Trigger $guardTrigger -Principal $principal -Settings $taskSettings -Description '保持 WorkBuddy 自启动关闭，恢复意外退出的轻量后台' -Force | Out-Null

$shellLink = New-Object -ComObject WScript.Shell
$iconPath = Join-Path $releaseDir 'BuddyHub-v2.ico'
$sourceIcon = Join-Path $appRoot 'desktop\assets\buddyhub.ico'
if (Test-Path -LiteralPath $sourceIcon) { Copy-Item -LiteralPath $sourceIcon -Destination $iconPath -Force }
$shortcutIcon = if (Test-Path -LiteralPath $iconPath) { $iconPath + ',0' } else { $exePath + ',0' }
$desktopPath = [Environment]::GetFolderPath('Desktop')
$desktopLink = Join-Path $desktopPath 'BuddyHub 本地 AI 管理.lnk'
$existingDesktopLink = Join-Path $desktopPath 'BuddyHub.lnk'
if (Test-Path -LiteralPath $existingDesktopLink) { $desktopLink = $existingDesktopLink }
$shortcut = $shellLink.CreateShortcut($desktopLink)
$shortcut.TargetPath = $exePath
$shortcut.WorkingDirectory = $releaseDir
$shortcut.Description = '统一管理账号签到、分区号池和本地反代'
$shortcut.IconLocation = $shortcutIcon
$shortcut.Save()
$menuPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\BuddyHub.lnk'
$menuLink = $shellLink.CreateShortcut($menuPath)
$menuLink.TargetPath = $exePath
$menuLink.WorkingDirectory = $releaseDir
$menuLink.IconLocation = $shortcutIcon
$menuLink.Save()
Write-Output '已安装桌面入口、登录后台启动、每日签到与自启守护。'

Start-ScheduledTask -TaskName "BuddyHub Background"
