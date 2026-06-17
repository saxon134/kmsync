@echo off
setlocal
set "APP_DIR=%~dp0"
start "" "%APP_DIR%kmsync.exe" core-service "%APP_DIR%configs\daemon.example.json"
