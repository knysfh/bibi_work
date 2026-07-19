!ifndef BIWORK_INSTALLER_UPDATE_VERIFY_NSH
!define BIWORK_INSTALLER_UPDATE_VERIFY_NSH

Var /GLOBAL BiWorkUninstallHadErrors
Var /GLOBAL BiWorkUninstallLogResult
Var /GLOBAL BiWorkVerifyResourceResult
Var /GLOBAL BiWorkUpdatedAppExitWaitResult
Var /GLOBAL BiWorkActiveMarkerExecResult
Var /GLOBAL BiWorkActiveMarkerResult

!define BIWORK_ACTIVE_INSTALLER_MARKER "biwork-installer-active.marker"

!macro BIWORK_BRING_UPDATED_INSTALLER_TO_FRONT
  ${If} ${isUpdated}
    BringToFront
    !insertmacro BIWORK_SLOG "event=updated-installer-foreground action=bring-to-front"
  ${EndIf}
!macroend

!macro BIWORK_WAIT_FOR_UPDATED_APP_EXIT
  ${If} ${isUpdated}
    !insertmacro BIWORK_SLOG "event=updated-app-exit-wait phase=start"
    StrCpy $BiWorkUpdatedAppExitWaitResult "0"

    nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
      $$ErrorActionPreference = 'SilentlyContinue'; \
      $$deadline = (Get-Date).AddSeconds(10); \
      $$target = [System.IO.Path]::GetFullPath((Join-Path '$INSTDIR' '${BIWORK_APP_EXECUTABLE_FILENAME}')); \
      do { \
        $$hits = @(Get-CimInstance -ClassName Win32_Process | Where-Object { \
          $$path = $$_.ExecutablePath; \
          if (-not $$path) { $$path = $$_.Path } \
          $$_.Name -ieq '${BIWORK_APP_EXECUTABLE_FILENAME}' -and $$path -and \
          [string]::Equals([System.IO.Path]::GetFullPath($$path), $$target, [System.StringComparison]::CurrentCultureIgnoreCase) \
        }); \
        if ($$hits.Count -eq 0) { exit 0 }; \
        Start-Sleep -Milliseconds 500; \
      } while ((Get-Date) -lt $$deadline); \
      exit 1 \
    }"`
    Pop $BiWorkUpdatedAppExitWaitResult

    ${If} $BiWorkUpdatedAppExitWaitResult != 0
      !insertmacro BIWORK_SLOG "event=updated-app-exit-wait phase=timeout action=stop"
      !insertmacro BIWORK_STOP_APP_PROCESSES
    ${EndIf}

    !insertmacro BIWORK_SLOG "event=updated-app-exit-wait phase=done result=$BiWorkUpdatedAppExitWaitResult"
  ${EndIf}
!macroend

!macro BIWORK_RECORD_ACTIVE_INSTALLER_MARKER
  nsExec::ExecToStack `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$marker = Join-Path $$env:TEMP '${BIWORK_ACTIVE_INSTALLER_MARKER}'; \
    if (-not (Test-Path -LiteralPath $$marker)) { Write-Output 'missing'; exit 0 }; \
    $$item = Get-Item -LiteralPath $$marker; \
    if ($$item.LastWriteTime -lt (Get-Date).AddHours(-2)) { Write-Output 'stale'; exit 0 }; \
    Write-Output 'active' \
  }"`
  Pop $BiWorkActiveMarkerExecResult
  Pop $BiWorkActiveMarkerResult
  ${If} $BiWorkActiveMarkerResult == "active"
    !insertmacro BIWORK_SLOG "event=installer-active-marker state=active"
  ${ElseIf} $BiWorkActiveMarkerResult == "stale"
    !insertmacro BIWORK_SLOG "event=installer-active-marker state=stale"
  ${Else}
    !insertmacro BIWORK_SLOG "event=installer-active-marker state=missing"
  ${EndIf}
!macroend

!macro BIWORK_WRITE_ACTIVE_INSTALLER_MARKER
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$marker = Join-Path $$env:TEMP '${BIWORK_ACTIVE_INSTALLER_MARKER}'; \
    Set-Content -LiteralPath $$marker -Encoding UTF8 -Value ('pid=' + $$PID + ';session=$BiWorkSessionId;started=' + (Get-Date -Format o)) \
  }"`
  Pop $BiWorkActiveMarkerResult
!macroend

!macro BIWORK_CLEAR_ACTIVE_INSTALLER_MARKER
  !ifndef BUILD_UNINSTALLER
    nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
      $$ErrorActionPreference = 'SilentlyContinue'; \
      Remove-Item -LiteralPath (Join-Path $$env:TEMP '${BIWORK_ACTIVE_INSTALLER_MARKER}') -Force \
    }"`
    Pop $BiWorkActiveMarkerResult
  !endif
!macroend

!macro BIWORK_OVERRIDE_SINGLE_INSTANCE
!macroend

!macro BIWORK_OVERRIDE_APP_CANNOT_BE_CLOSED_MESSAGE
  !pragma warning disable 6030
  LangString appCannotBeClosed 1033 "${BIWORK_MSG_APP_CANNOT_BE_CLOSED_ZH}$\r$\n$\r$\n${BIWORK_MSG_BLOCK_SEPARATOR}$\r$\n$\r$\n${BIWORK_MSG_APP_CANNOT_BE_CLOSED_EN}"
  LangString appCannotBeClosed 2052 "${BIWORK_MSG_APP_CANNOT_BE_CLOSED_ZH}$\r$\n$\r$\n${BIWORK_MSG_BLOCK_SEPARATOR}$\r$\n$\r$\n${BIWORK_MSG_APP_CANNOT_BE_CLOSED_EN}"
  !pragma warning default 6030
!macroend

!macro BIWORK_INSTALLER_CUSTOM_HEADER
  !insertmacro BIWORK_OVERRIDE_SINGLE_INSTANCE
  !insertmacro BIWORK_OVERRIDE_APP_CANNOT_BE_CLOSED_MESSAGE
!macroend

!macro BIWORK_RELEASE_INSTALL_DIR_OUTDIR
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  StrCpy $BiWorkCurrentOutDir "$PLUGINSDIR"
!macroend

!macro BIWORK_INSTALLER_PREINIT
  !ifdef BUILD_UNINSTALLER
    StrCpy $BiWorkSessionId ""
    StrCpy $BiWorkIsUpdated "0"
    StrCpy $BiWorkSessionLogResult ""
    StrCpy $BiWorkSessionLogPath "$TEMP\${BIWORK_FALLBACK_LOG}"
    StrCpy $BiWorkUninstallHadErrors "0"
    StrCpy $BiWorkUninstallLogResult ""
    StrCpy $BiWorkVerifyResourceResult ""
    StrCpy $BiWorkUpdatedAppExitWaitResult ""
    StrCpy $BiWorkActiveMarkerExecResult ""
    StrCpy $BiWorkActiveMarkerResult ""
    StrCpy $BiWorkStopResult ""
    StrCpy $BiWorkLockerListZh ""
    StrCpy $BiWorkLockerListEn ""
  !else
    !insertmacro BIWORK_RELEASE_INSTALL_DIR_OUTDIR
    !insertmacro BIWORK_SESSION_BEGIN
    !insertmacro BIWORK_SLOG "event=installer-outdir-release outDir=$BiWorkCurrentOutDir instDir=$INSTDIR"
    !insertmacro BIWORK_BRING_UPDATED_INSTALLER_TO_FRONT
    !insertmacro BIWORK_RECORD_ACTIVE_INSTALLER_MARKER
    !insertmacro BIWORK_WRITE_ACTIVE_INSTALLER_MARKER
  !endif
!macroend

!macro BIWORK_VERIFY_REQUIRED_FILE _PATH _LABEL
  ${IfNot} ${FileExists} "${_PATH}"
    !insertmacro BIWORK_LOG_EVENT "verify-required-file missing label=${_LABEL} path=${_PATH}"
    !insertmacro BIWORK_FAIL_UX \
      "${BIWORK_E_CORE_APP_FILES_INCOMPLETE}" \
      "verify-required-file missing label=${_LABEL} path=${_PATH}" \
      "${BIWORK_MSG_VERIFY_REQUIRED_FILE_ZH} ${_LABEL}" \
      "${BIWORK_MSG_VERIFY_REQUIRED_FILE_EN} ${_LABEL}" \
      "${BIWORK_MSG_VERIFY_REQUIRED_FILE_ACTION_ZH}" \
      "${BIWORK_MSG_VERIFY_REQUIRED_FILE_ACTION_EN}" \
      "verify-required-file missing label=${_LABEL} path=${_PATH}" \
      "verify-required-file missing label=${_LABEL} path=${_PATH}"
  ${Else}
    !insertmacro BIWORK_LOG_EVENT "verify-required-file ok label=${_LABEL} path=${_PATH}"
  ${EndIf}
!macroend

!macro BIWORK_VERIFY_CORE_APP_FILES
  !insertmacro BIWORK_LOG_EVENT "verify-install start instDir=$INSTDIR"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\BiWork.exe" "BiWork.exe"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\ffmpeg.dll" "ffmpeg.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\libEGL.dll" "libEGL.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\libGLESv2.dll" "libGLESv2.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\d3dcompiler_47.dll" "d3dcompiler_47.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\dxcompiler.dll" "dxcompiler.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\dxil.dll" "dxil.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\vk_swiftshader.dll" "vk_swiftshader.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\vulkan-1.dll" "vulkan-1.dll"
  !insertmacro BIWORK_VERIFY_REQUIRED_FILE "$INSTDIR\resources\app.asar" "resources\app.asar"
!macroend

!macro customInstall
  !insertmacro BIWORK_VERIFY_CORE_APP_FILES
  !insertmacro BIWORK_LOG_EVENT "verify-install ok instDir=$INSTDIR"
  !insertmacro BIWORK_CLEAR_ACTIVE_INSTALLER_MARKER
  !insertmacro BIWORK_SESSION_SUCCESS
!macroend

!endif
