; FPS Viewer — Windows installer (NSIS 3, Unicode).
;
; Build with release.sh, or by hand:
;   makensis -DVERSION=1.0.0 -DEXE=/abs/path/FpsViewer.exe -DOUTFILE=/abs/path/FpsViewer-Setup-x64.exe installer/FpsViewer.nsi
;
; What it does:
;   * installs FpsViewer.exe to C:\Program Files\FPS Viewer (admin-only, which is what makes
;     the no-prompt launch safe — see sys::is_installed)
;   * Start menu shortcut, entry in Settings → Apps with an uninstaller
;   * registers the "FPS Viewer Launcher" task (opening the app needs no UAC prompt) and,
;     unless unticked, the "FPS Viewer" sign-in task
; Silent install (/S, used by winget) keeps the defaults, i.e. starts with Windows.

Unicode true
ManifestDPIAware true
ManifestSupportedOS Win10

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "Sections.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "1.0.0"
!endif
!ifndef EXE
  !error "Pass -DEXE=<path to the built FpsViewer.exe>"
!endif
!ifndef OUTFILE
  !define OUTFILE "FpsViewer-Setup-x64.exe"
!endif
!ifndef ICON
  !define ICON "${__FILEDIR__}\..\assets\app.ico"
!endif

!define APP        "FPS Viewer"
!define PUBLISHER  "FPS Viewer"
!define EXE_NAME   "FpsViewer.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\FPSViewer"

Name "${APP}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\${APP}"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
BrandingText " "
ShowInstDetails nevershow
ShowUninstDetails nevershow

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName"     "${APP}"
VIAddVersionKey "CompanyName"     "${PUBLISHER}"
VIAddVersionKey "FileDescription" "${APP} Setup"
VIAddVersionKey "FileVersion"     "${VERSION}"
VIAddVersionKey "ProductVersion"  "${VERSION}"
VIAddVersionKey "LegalCopyright"  "Copyright © 2026 ${PUBLISHER}"

; ───────────────────────── pages ─────────────────────────

!define MUI_ICON   "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC

!define MUI_WELCOMEPAGE_TITLE "Install ${APP} ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "A featherweight frame-rate overlay for Windows.$\r$\n$\r$\nIt shows your game's FPS, CPU and memory use in a small card on top of the game, and lives in the tray.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_HEADER_TEXT "Options"
!define MUI_PAGE_HEADER_SUBTEXT "Choose how ${APP} starts."
!define MUI_COMPONENTSPAGE_TEXT_TOP "${APP} will be installed to $INSTDIR."
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TITLE "${APP} is installed"
!define MUI_FINISHPAGE_TEXT "Open it any time from the Start menu. While it runs it lives in the tray; press Alt+Shift+O to open its settings."
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE_NAME}"
!define MUI_FINISHPAGE_RUN_TEXT "Open ${APP} now"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ───────────────────────── install ─────────────────────────

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${APP} needs 64-bit Windows 10 or 11."
    Abort
  ${EndIf}
  SetRegView 64
  ; Fixed location: the silent, elevated launch is only safe from an admin-only folder.
  StrCpy $INSTDIR "$PROGRAMFILES64\${APP}"
FunctionEnd

Section "${APP}" SecApp
  SectionIn RO
  ; Close a running copy so its exe can be replaced (updates).
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${EXE_NAME}'
  Pop $0
  Sleep 400

  SetOutPath "$INSTDIR"
  File "/oname=${EXE_NAME}" "${EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  SetShellVarContext all
  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE_NAME}" "" "$INSTDIR\${EXE_NAME}" 0

  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayName"          "${APP}"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayVersion"       "${VERSION}"
  WriteRegStr   HKLM "${UNINST_KEY}" "Publisher"            "${PUBLISHER}"
  WriteRegStr   HKLM "${UNINST_KEY}" "DisplayIcon"          '"$INSTDIR\${EXE_NAME}",0'
  WriteRegStr   HKLM "${UNINST_KEY}" "InstallLocation"      "$INSTDIR"
  WriteRegStr   HKLM "${UNINST_KEY}" "UninstallString"      '"$INSTDIR\Uninstall.exe"'
  WriteRegStr   HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Start ${APP} when I sign in" SecStartup
  ; Registered below, once we know whether this is ticked.
SectionEnd

Section "-Scheduled tasks"
  ${If} ${SectionIsSelected} ${SecStartup}
    StrCpy $1 "--startup"
  ${Else}
    StrCpy $1 "--no-startup"
  ${EndIf}
  nsExec::Exec '"$INSTDIR\${EXE_NAME}" --install $1'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Couldn't register the launch tasks (exit $0). The app still works, with a UAC prompt each time it opens."
  ${EndIf}
SectionEnd

; ───────────────────────── uninstall ─────────────────────────

Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${EXE_NAME}'
  Pop $0
  nsExec::Exec '"$INSTDIR\${EXE_NAME}" --uninstall'
  Pop $0
  Sleep 400

  Delete "$INSTDIR\${EXE_NAME}"
  Delete "$INSTDIR\.task-*.xml"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir  "$INSTDIR"

  SetShellVarContext all
  Delete "$SMPROGRAMS\${APP}.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"
  ; Settings in %APPDATA%\FpsViewer are kept, like most apps do, for a later reinstall.
SectionEnd
