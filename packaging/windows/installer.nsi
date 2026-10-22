; maclean Windows 安装器（NSIS）
; per-user 安装到 %LOCALAPPDATA%\Programs\maclean，无需管理员提权
; 自动把安装目录写入 HKCU\Environment\Path（当前会话 + 新会话均生效）
; 生成卸载器 + 卸载时清理 PATH 条目
; 用法: makensis /DVERSION=x.y.z /DEXE_PATH=dist\maclean.exe installer.nsi

!include "MUI2.nsh"
!include "WinMessages.nsh"
!include "LogicLib.nsh"
!include "StrFunc.nsh"
${StrStr}
${StrRep}
${UnStrStr}
${UnStrRep}

!ifndef APP_VERSION
  !define APP_VERSION "0.2.0"
!endif
!ifndef EXE_PATH
  !error "EXE_PATH 未定义：请用 /DEXE_PATH=dist\maclean.exe 指定 maclean.exe"
!endif

!define APP_NAME "maclean"
!define APP_EXE "maclean.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\maclean"

Name "maclean ${APP_VERSION}"
OutFile "maclean-setup-${APP_VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\maclean"
RequestExecutionLevel user
Unicode true

; MUI 页面
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; 语言
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${EXE_PATH}"

  ; ---- 追加 PATH（HKCU，免提权）----
  ReadRegStr $0 HKCU "Environment" "Path"
  ${StrStr} $1 "$0" "$INSTDIR"
  StrCmp $1 "" 0 path_done
    StrCmp $0 "" 0 path_append
      StrCpy $0 "$INSTDIR"
      Goto path_write
  path_append:
      StrCpy $0 "$0;$INSTDIR"
  path_write:
      WriteRegExpandStr HKCU "Environment" "Path" "$0"
      ; 广播环境变更，当前会话立即生效（新开窗口也生效）
      SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  path_done:

  ; ---- 卸载器 + 卸载信息 ----
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "maclean ${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "maclean"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
SectionEnd

Section "Uninstall"
  ; 移除 PATH 中的安装目录
  ReadRegStr $0 HKCU "Environment" "Path"
  ${UnStrRep} $0 "$0" "$INSTDIR;" ""
  ${UnStrRep} $0 "$0" ";$INSTDIR" ""
  ${UnStrRep} $0 "$0" "$INSTDIR" ""
  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000

  ; 删除文件与注册表
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\${APP_EXE}"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
