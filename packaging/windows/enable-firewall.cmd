@echo off
setlocal
set "APP_DIR=%~dp0"
set "APP_EXE=%APP_DIR%kmsync.exe"

net session >nul 2>&1
if not "%errorlevel%"=="0" (
  powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
  exit /b
)

netsh advfirewall firewall delete rule name="KMSync Input Sync" >nul 2>&1
netsh advfirewall firewall add rule name="KMSync Input Sync" dir=in action=allow program="%APP_EXE%" enable=yes protocol=UDP localport=24800
pause
