@echo off
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0tools\install.ps1"
if errorlevel 1 (
  echo Installation failed. Please check the error above.
  pause
  exit /b 1
)
start "" "%~dp0BuddyHub.exe"
