Unicode True
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef SOURCE_DIR
  !error "SOURCE_DIR is required"
!endif
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "HeiGate-Windows-x64-Setup.exe"
!endif

Name "HeiGate"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\HeiGate"
InstallDirRegKey HKCU "Software\HeiGate" "InstallDir"
Icon "AppIcon.ico"
UninstallIcon "AppIcon.ico"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "HeiGate"
VIAddVersionKey "CompanyName" "HeiGate"
VIAddVersionKey "FileDescription" "HeiGate local AI gateway installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!define MUI_ABORTWARNING
!define MUI_ICON "AppIcon.ico"
!define MUI_UNICON "AppIcon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\HeiGate.exe"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

; Closing the window only hides HeiGate to the tray, so an upgrade usually finds
; it running. Overwriting a running exe fails with "Error opening file for
; writing", so stop the shell and its gateway child before touching files.
!macro StopHeiGate
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /T /IM HeiGate.exe'
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /IM heigate-gateway.exe'
  ; Give Windows a moment to release the image file handles.
  Sleep 1000
!macroend

Section "HeiGate" SEC_MAIN
  SetShellVarContext current
  !insertmacro StopHeiGate
  SetOutPath "$INSTDIR"
  File /r "${SOURCE_DIR}\*"

  WriteRegStr HKCU "Software\HeiGate" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\HeiGate"
  CreateShortcut "$SMPROGRAMS\HeiGate\HeiGate.lnk" "$INSTDIR\HeiGate.exe"
  CreateShortcut "$SMPROGRAMS\HeiGate\卸载 HeiGate.lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "DisplayName" "HeiGate"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "DisplayIcon" "$INSTDIR\HeiGate.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "Publisher" "HeiGate"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "HeiGate"
  !insertmacro StopHeiGate
  Delete "$SMPROGRAMS\HeiGate\HeiGate.lnk"
  Delete "$SMPROGRAMS\HeiGate\卸载 HeiGate.lnk"
  RMDir "$SMPROGRAMS\HeiGate"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HeiGate"
  DeleteRegKey HKCU "Software\HeiGate"
SectionEnd
