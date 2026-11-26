Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

!define APPDIR "$LOCALAPPDATA\Programs\DeskIsle"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\DeskIsle"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
Name "DeskIsle ${VERSION}"
OutFile "${RELEASE_DIR}\DeskIsle-Setup-x64.exe"
InstallDir "${APPDIR}"
RequestExecutionLevel user
SetCompressor /SOLID lzma
AllowSkipFiles off

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\deskisle.exe"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

LangString InstallFailed ${LANG_SIMPCHINESE} "安装未完成。请退出 DeskIsle 并确认当前用户可以写入安装目录，然后重试。"
LangString InstallFailed ${LANG_ENGLISH} "Installation failed. Exit DeskIsle, check write access to the install folder and try again."
LangString UninstallFailed ${LANG_SIMPCHINESE} "卸载未完成。请退出 DeskIsle 后重试。"
LangString UninstallFailed ${LANG_ENGLISH} "Uninstall failed. Exit DeskIsle and try again."
LangString SetupBusy ${LANG_SIMPCHINESE} "请等待另一个 DeskIsle 安装或卸载操作完成后重试。"
LangString SetupBusy ${LANG_ENGLISH} "Another DeskIsle setup is running. Wait for it to finish and try again."

; Keep the mutex handle until this installer or uninstaller exits.
!macro LockSetup
    System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Local\DeskIsle.Setup") p.r0 ?e'
    Pop $1
    ${If} $0 = 0
    ${OrIf} $1 = 183 ; ERROR_ALREADY_EXISTS
        MessageBox MB_OK|MB_ICONSTOP "$(SetupBusy)" /SD IDOK
        SetErrorLevel 1
        Abort
    ${EndIf}
!macroend

Function .onInit
    ${IfNot} ${IsNativeAMD64}
        MessageBox MB_OK|MB_ICONSTOP "DeskIsle requires Windows x64." /SD IDOK
        Abort
    ${EndIf}
    ; The updater and the uninstall registration share this per-user location.
    StrCpy $INSTDIR "${APPDIR}"
    !insertmacro LockSetup
FunctionEnd

Function un.onInit
    !insertmacro LockSetup
FunctionEnd

Section "DeskIsle"
    SetRegView 64
    SetOutPath "$INSTDIR"
    ; Extract first so a locked file or an interrupted extraction leaves the old EXE intact.
    GetTempFileName $0 "$INSTDIR"
    IfErrors failed
    File "/oname=$0" "${RELEASE_DIR}\deskisle.exe"
    System::Call 'kernel32::MoveFileExW(w r0, w "$INSTDIR\deskisle.exe", i 1)i.r1'
    ${If} $1 = 0
        Delete "$0"
        Goto failed
    ${EndIf}
    WriteUninstaller "$INSTDIR\uninstall.exe"
    CreateShortcut "$SMPROGRAMS\DeskIsle.lnk" "$INSTDIR\deskisle.exe"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "DeskIsle"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
    IfErrors failed done
failed:
    MessageBox MB_OK|MB_ICONSTOP "$(InstallFailed)" /SD IDOK
    SetErrorLevel 1
    Abort
done:
SectionEnd

Section "Uninstall"
    SetRegView 64
    StrCpy $INSTDIR "${APPDIR}"
    ${If} ${FileExists} "$INSTDIR\deskisle.exe"
        Delete "$INSTDIR\deskisle.exe"
        IfErrors failed
    ${EndIf}
    ReadRegStr $0 HKCU "${RUN_KEY}" "DeskIsle"
    ClearErrors
    ${If} $0 == '"$INSTDIR\deskisle.exe"'
        DeleteRegValue HKCU "${RUN_KEY}" "DeskIsle"
    ${EndIf}
    ${If} ${FileExists} "$SMPROGRAMS\DeskIsle.lnk"
        Delete "$SMPROGRAMS\DeskIsle.lnk"
    ${EndIf}
    IfErrors failed
    Delete "$INSTDIR\uninstall.exe"
    IfErrors failed
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    IfErrors failed
    RMDir "$INSTDIR"
    Goto done
failed:
    MessageBox MB_OK|MB_ICONSTOP "$(UninstallFailed)" /SD IDOK
    SetErrorLevel 1
    Abort
done:
SectionEnd
