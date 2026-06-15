!ifndef APP_VERSION
  !define APP_VERSION "0.1.0"
!endif
!ifndef APP_TARGET
  !define APP_TARGET "x86_64-pc-windows-msvc"
!endif

!define APP_NAME "KMSync"
!define APP_PUBLISHER "KMSync"
!define APP_EXE "kmsync.exe"
!define SERVICE_NAME "KMSyncCoreService"
!define SERVICE_DISPLAY_NAME "KMSync Core Service"

Name "${APP_NAME}"
OutFile "..\..\dist\windows\kmsync-${APP_VERSION}-windows-x64-setup.exe"
InstallDir "$PROGRAMFILES64\KMSync"
RequestExecutionLevel admin

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetOutPath "$INSTDIR"
  File "..\..\target\${APP_TARGET}\release\${APP_EXE}"
  SetOutPath "$INSTDIR\configs"
  File "..\..\configs\mac-to-windows.profile.json"
  File "..\..\configs\windows-to-mac.profile.json"
  SetOutPath "$INSTDIR\docs"
  File "..\..\docs\USER_GUIDE.md"
  File /oname=daemon.example.json "..\..\configs\daemon.example.json"

  CreateDirectory "$SMPROGRAMS\KMSync"
  CreateShortCut "$SMPROGRAMS\KMSync\KMSync permissions and guide.lnk" "$INSTDIR\docs\USER_GUIDE.md"
  CreateShortCut "$SMPROGRAMS\KMSync\KMSync status.lnk" "$INSTDIR\${APP_EXE}" "status"
  CreateShortCut "$SMPROGRAMS\KMSync\KMSync info.lnk" "$INSTDIR\${APP_EXE}" "info"
  nsExec::ExecToLog 'sc.exe create "${SERVICE_NAME}" binPath= "\"$INSTDIR\${APP_EXE}\" windows-service" DisplayName= "${SERVICE_DISPLAY_NAME}" start= auto'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "KMSync" '"$INSTDIR\${APP_EXE}" core-service'
  ExecShell "open" "$INSTDIR\${APP_EXE}" "core-service" SW_HIDE

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\KMSync" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\KMSync" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\KMSync" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\KMSync" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'sc.exe stop "${SERVICE_NAME}"'
  nsExec::ExecToLog 'sc.exe delete "${SERVICE_NAME}"'
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\configs\mac-to-windows.profile.json"
  Delete "$INSTDIR\configs\windows-to-mac.profile.json"
  Delete "$INSTDIR\docs\USER_GUIDE.md"
  Delete "$INSTDIR\docs\daemon.example.json"
  Delete "$SMPROGRAMS\KMSync\KMSync permissions and guide.lnk"
  Delete "$SMPROGRAMS\KMSync\KMSync status.lnk"
  Delete "$SMPROGRAMS\KMSync\KMSync info.lnk"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR\configs"
  RMDir "$INSTDIR\docs"
  RMDir "$SMPROGRAMS\KMSync"
  RMDir "$INSTDIR"
  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "KMSync"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\KMSync"
SectionEnd
