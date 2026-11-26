; AxiDPS per-user installer. Build:
;   makensis -DVERSION=<v> -DSTAGE=<dir> [-DOUTDIR=<dir>] [-DSIGN_PS1=<sign.ps1>] packaging/windows/axidps.nsi
; With SIGN_PS1 (the release workflow, which has the signing secrets in the environment) the
; uninstaller is signed as it is built; without it (a local build) it is left unsigned.
Unicode true
!ifndef VERSION
  !error "VERSION is required: makensis -DVERSION=<v> -DSTAGE=<dir> axidps.nsi"
!endif
!ifndef STAGE
  !error "STAGE is required: makensis -DVERSION=<v> -DSTAGE=<dir> axidps.nsi"
!endif
!ifndef OUTDIR
  !define OUTDIR "."
!endif

; NSIS >= 3.08: runs on the generated uninstaller before it is embedded; a non-zero exit (a
; failed or invalid signature) fails the build.
!ifdef SIGN_PS1
  !uninstfinalize 'pwsh -NoProfile -NonInteractive -File "${SIGN_PS1}" -Files "%1"' = 0
!endif

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

Name "AxiDPS"
OutFile "${OUTDIR}/axidps-${VERSION}-Setup.exe"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\axidps"
InstallDirRegKey HKCU "Software\axidps" "InstallDir"
SetCompressor /SOLID lzma

!define MUI_ICON "..\icon.ico"
!define MUI_UNICON "..\icon.ico"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\axidps"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

!macro KillAll
  nsExec::Exec 'taskkill /F /IM axidps.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /IM axidps-core.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /IM axidps-overlay.exe'
  Pop $0
  ; Wait (up to 10 s) until they are gone, so File /r does not hit a file that is still locked.
  StrCpy $R9 0
  ${Do}
    nsExec::Exec 'cmd /c tasklist /NH | findstr /I /B /C:"axidps.exe " /C:"axidps-core.exe " /C:"axidps-overlay.exe "'
    Pop $0
    ${If} $0 != 0
      ${Break}
    ${EndIf}
    Sleep 200
    IntOp $R9 $R9 + 1
  ${LoopUntil} $R9 >= 50
!macroend

; Asks a running app to stop the way an update does, then kills what is left. The launcher
; stops on launcher.stop (core finishes the fight in progress, then everything exits); core
; also stops on its own core.stop, which covers a launcher from before launcher.stop (it
; restarts core, but that one has no fight to lose). Waits 40 rounds of a 200 ms sleep plus a
; tasklist run, about 15-25 s in all (the launcher gives core 10 s), then falls back to KillAll.
; A launcher from before launcher.stop never exits by itself, so it always waits that long.
!define CONFIG_DIR "$APPDATA\axidps"
!macro StopApp
  nsExec::Exec 'cmd /c tasklist /NH | findstr /I /B /C:"axidps.exe " /C:"axidps-core.exe "'
  Pop $0
  ${If} $0 == 0
  ${AndIf} ${FileExists} "${CONFIG_DIR}\*.*"
    FileOpen $R8 "${CONFIG_DIR}\launcher.stop" w
    FileClose $R8
    FileOpen $R8 "${CONFIG_DIR}\core.stop" w
    FileClose $R8
    StrCpy $R9 0
    ${Do}
      Sleep 200
      nsExec::Exec 'cmd /c tasklist /NH | findstr /I /B /C:"axidps.exe " /C:"axidps-core.exe "'
      Pop $0
      ${If} $0 != 0
        ${Break}
      ${EndIf}
      IntOp $R9 $R9 + 1
    ${LoopUntil} $R9 >= 40
    Delete "${CONFIG_DIR}\launcher.stop"
    Delete "${CONFIG_DIR}\core.stop"
  ${EndIf}
  !insertmacro KillAll
!macroend

; Starts the installed app again, after StopApp stopped it and the install or uninstall failed.
!macro Relaunch
  ${If} ${FileExists} "$INSTDIR\axidps.exe"
    ${If} ${Silent}
      System::Call 'Kernel32::SetEnvironmentVariable(t "AXIDPS_SILENT", t "1")'
    ${EndIf}
    Exec '"$INSTDIR\axidps.exe"'
  ${EndIf}
!macroend

; Refuses (before anything is killed or changed) while Guild Wars 2 runs.
!macro GameGuard un
Function ${un}GameGuard
  loop:
    nsExec::Exec 'cmd /c tasklist /FI "IMAGENAME eq Gw2-64.exe" /NH | find /I "Gw2-64.exe"'
    Pop $0
    ${If} $0 != 0
      Return
    ${EndIf}
    ${If} ${Silent}
      SetErrorLevel 2
      Abort
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Close Guild Wars 2 first." IDRETRY loop
    SetErrorLevel 2
    Abort
FunctionEnd
!macroend
!insertmacro GameGuard ""
!insertmacro GameGuard "un."

; An aborted or failed install (Guild Wars 2 started meanwhile, a locked file) must not leave
; axidps stopped for the session: the silent self-update has already closed the old launcher.
; A launcher that is still running makes the new one exit at once.
Function .onInstFailed
  ${GetParameters} $1
  ${GetOptions} $1 "/LAUNCH=" $2
  ${If} $2 != "0"
    !insertmacro Relaunch
  ${EndIf}
FunctionEnd

Section "Install"
  Call GameGuard
  !insertmacro StopApp

  ; drop stale Servo/page files from earlier versions
  RMDir /r "$INSTDIR\overlay"
  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\axidps" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "AxiDPS"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "darkharasho"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\axidps.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ; Start at login until the game can start the launcher: the launcher deletes this value once
  ; the DLL in the game folder starts it (and puts it back if that DLL is replaced by an older one).
  WriteRegStr HKCU "${RUN_KEY}" "axidps" '"$INSTDIR\axidps.exe"'

  ; AXIDPS_SILENT stays set, so the relaunched app inherits it below.
  ${If} ${Silent}
    System::Call 'Kernel32::SetEnvironmentVariable(t "AXIDPS_SILENT", t "1")'
  ${EndIf}
  ExecWait '"$INSTDIR\axidps.exe" --install-game' $0

  ${GetParameters} $1
  ${GetOptions} $1 "/LAUNCH=" $2
  ${If} $2 != "0"
    Exec '"$INSTDIR\axidps.exe"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  Call un.GameGuard
  !insertmacro StopApp
  ClearErrors
  ExecWait '"$INSTDIR\axidps.exe" --uninstall' $0
  ${If} ${Errors}
  ${OrIf} $0 != 0
    ; Nothing was removed; StopApp stopped the app, so start it again.
    !insertmacro Relaunch
    ${IfNot} ${Silent}
      MessageBox MB_OK|MB_ICONSTOP "Restoring the game folder failed, so AxiDPS was not removed. Close Guild Wars 2 and try again."
    ${EndIf}
    SetErrorLevel 1
    Abort
  ${EndIf}
  SetOutPath "$TEMP"
  RMDir /r "$INSTDIR"
  DeleteRegValue HKCU "${RUN_KEY}" "axidps"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\axidps"
SectionEnd
