!ifndef BIWORK_INSTALLER_REPAIR_HEAL_NSH
!define BIWORK_INSTALLER_REPAIR_HEAL_NSH

Var /GLOBAL BiWorkRegistryInstallIsValid
Var /GLOBAL BiWorkInnerFailureSummary
Var /GLOBAL BiWorkInnerRootCode
Var /GLOBAL BiWorkInnerFailureReadResult

!macro BIWORK_READ_LAST_INNER_FAILURE
  InitPluginsDir
  StrCpy $BiWorkInnerRootCode ""
  StrCpy $BiWorkInnerFailureSummary "No specific locking process was identified. Close BiWork, terminals, editors, and file managers opened in the install folder."
  nsExec::ExecToStack `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$logPath = '$BiWorkSessionLogPath'; \
    $$summary = 'No specific locking process was identified. Close BiWork, terminals, editors, and file managers opened in the install folder.'; \
    $$code = ''; \
    if ($$logPath -and (Test-Path -LiteralPath $$logPath)) { \
      $$events = @(Get-Content -LiteralPath $$logPath -ErrorAction SilentlyContinue | ForEach-Object { try { $$_ | ConvertFrom-Json } catch { $$null } } | Where-Object { $$_ }); \
      $$failure = @($$events | Where-Object { $$_.event -eq 'failure' -and $$_.updated -eq $$true } | Select-Object -Last 1)[0]; \
      if (-not $$failure) { $$failure = @($$events | Where-Object { $$_.event -eq 'failure' } | Select-Object -Last 1)[0] }; \
      if ($$failure) { \
        $$code = ([string]$$failure.code).Trim(); \
        $$phase = ([string]$$failure.phase).Trim(); \
        $$path = ([string]$$failure.failedPath).Trim(); \
        $$blocking = ''; \
        $$processes = @($$failure.blockingProcesses); \
        if ($$processes.Count -gt 0) { $$blocking = (@($$processes | ForEach-Object { if ($$_.pid) { [string]$$_.name + '(' + [string]$$_.pid + ')' } else { [string]$$_.name } }) -join ', ') }; \
        if (-not $$blocking) { $$blocking = ([string]$$failure.message).Trim() }; \
        if (-not $$blocking) { $$blocking = 'Windows did not identify a specific locking process. Close terminals, editors, and file managers opened in the install folder.' }; \
        $$parts = @('- Outer installer: previous uninstaller exited with code $R0', ('- Inner failure: ' + $$code + ' phase ' + $$phase)); \
        if ($$path) { $$parts += ('- File or folder: ' + $$path) }; \
        $$parts += ('- Blocking process: ' + $$blocking); \
        $$summary = $$parts -join [Environment]::NewLine; \
      } \
    }; \
    if (-not $$code) { $$code = '-----' }; \
    [Console]::Out.Write($$code + '|' + $$summary) \
  }"`
  Pop $BiWorkInnerFailureReadResult
  Pop $BiWorkInnerFailureReadResult
  StrCpy $BiWorkInnerRootCode $BiWorkInnerFailureReadResult 5
  ${If} $BiWorkInnerRootCode == "-----"
    StrCpy $BiWorkInnerRootCode ""
  ${EndIf}
  StrCpy $BiWorkInnerFailureSummary $BiWorkInnerFailureReadResult 4096 6
!macroend

!macro BIWORK_LOG_UNINSTALLER_REPAIR _PHASE
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$path = '$INSTDIR\${UNINSTALL_FILENAME}'; \
    $$item = Get-Item -LiteralPath $$path -ErrorAction SilentlyContinue; \
    $$version = if ($$item) { $$item.VersionInfo.ProductVersion } else { '' }; \
    $$length = if ($$item) { $$item.Length } else { '' }; \
    $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = '$BiWorkSessionId'; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = 'uninstaller-repair'; phase = '${_PHASE}'; path = $$path; exists = [bool]$$item; productVersion = $$version; length = $$length }; \
    Add-Content -LiteralPath $$log -Encoding UTF8 -Value ($$payload | ConvertTo-Json -Compress -Depth 8) \
  }"`
  Pop $BiWorkRepairLogResult
!macroend

!macro BIWORK_REPAIR_INSTALLED_UNINSTALLER
  Var /GLOBAL BiWorkInstalledUninstaller
  Var /GLOBAL BiWorkBundledUninstaller
  Var /GLOBAL BiWorkRepairLogResult

  !insertmacro BIWORK_LOG_UNINSTALLER_REPAIR "before"
  StrCpy $BiWorkInstalledUninstaller "$INSTDIR\${UNINSTALL_FILENAME}"

  InitPluginsDir
  StrCpy $BiWorkBundledUninstaller "$PLUGINSDIR\BiWork-fixed-uninstaller.exe"
  SetOverwrite on
  File "/oname=$PLUGINSDIR\BiWork-fixed-uninstaller.exe" "${UNINSTALLER_OUT_FILE}"

  ${If} ${FileExists} "$BiWorkInstalledUninstaller"
    ClearErrors
    CopyFiles /SILENT "$BiWorkBundledUninstaller" "$BiWorkInstalledUninstaller"
    ${If} ${Errors}
      !insertmacro BIWORK_LOG_UNINSTALLER_REPAIR "copy-failed-retry"
      !insertmacro BIWORK_STOP_APP_PROCESSES
      Sleep 1000

      ClearErrors
      CopyFiles /SILENT "$BiWorkBundledUninstaller" "$BiWorkInstalledUninstaller"
      ${If} ${Errors}
        ${If} ${FileExists} "$BiWorkBundledUninstaller"
          !insertmacro BIWORK_LOG_UNINSTALLER_REPAIR "copy-failed-using-bundled"
          !insertmacro BIWORK_LOG_EVENT "event=uninstaller-repair phase=copy-failed-using-bundled"
        ${Else}
          !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL ${BIWORK_E_UNINSTALLER_COPY_OR_REBUILD_FAILED} "uninstaller-repair copy-failed-retry-bundled-missing" "${BIWORK_MSG_UNINSTALLER_COPY_LOCKED_EN}" "${BIWORK_MSG_UNINSTALLER_COPY_LOCKED_ZH}" "${BIWORK_MSG_UNINSTALLER_REPAIR_ACTION_EN}" "${BIWORK_MSG_UNINSTALLER_REPAIR_ACTION_ZH}"
        ${EndIf}
      ${Else}
        !insertmacro BIWORK_LOG_UNINSTALLER_REPAIR "after-copy-retry"
      ${EndIf}
    ${Else}
      !insertmacro BIWORK_LOG_UNINSTALLER_REPAIR "after-copy"
    ${EndIf}
  ${Else}
    ClearErrors
    CopyFiles /SILENT "$BiWorkBundledUninstaller" "$BiWorkInstalledUninstaller"
    ${If} ${Errors}
      !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL ${BIWORK_E_UNINSTALLER_COPY_OR_REBUILD_FAILED} "uninstaller-repair rebuild-failed" "${BIWORK_MSG_UNINSTALLER_REBUILD_FAILED_EN}" "${BIWORK_MSG_UNINSTALLER_REBUILD_FAILED_ZH}" "${BIWORK_MSG_UNINSTALLER_REPAIR_ACTION_EN}" "${BIWORK_MSG_UNINSTALLER_REPAIR_ACTION_ZH}"
    ${EndIf}

    ${IfNot} ${FileExists} "$BiWorkInstalledUninstaller"
      !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL ${BIWORK_E_UNINSTALLER_COPY_OR_REBUILD_FAILED} "uninstaller-repair rebuild-missing-after-copy" "${BIWORK_MSG_UNINSTALLER_REBUILD_MISSING_EN}" "${BIWORK_MSG_UNINSTALLER_REBUILD_MISSING_ZH}" "${BIWORK_MSG_UNINSTALLER_REPAIR_ACTION_EN}" "${BIWORK_MSG_UNINSTALLER_REPAIR_ACTION_ZH}"
    ${EndIf}

    !insertmacro BIWORK_LOG_UNINSTALLER_REPAIR "rebuilt"
    !insertmacro BIWORK_LOG_EVENT "event=uninstaller-repair phase=rebuilt"
  ${EndIf}
!macroend

!macro BIWORK_HEAL_INSTALL_REGISTRY
  Var /GLOBAL BiWorkRegInstallLocation
  Var /GLOBAL BiWorkRegUninstallString
  Var /GLOBAL BiWorkRegInstallExe

  StrCpy $BiWorkRegistryInstallIsValid "0"

  ReadRegStr $BiWorkRegInstallLocation SHCTX "${INSTALL_REGISTRY_KEY}" "InstallLocation"
  ReadRegStr $BiWorkRegUninstallString SHCTX "${UNINSTALL_REGISTRY_KEY}" "UninstallString"

  ${If} $BiWorkRegInstallLocation == ""
    !insertmacro BIWORK_LOG_EVENT "event=registry-heal phase=missing-install-location uninstallString=$BiWorkRegUninstallString"
    !insertmacro BIWORK_CLEAR_INSTALL_REGISTRY "missing-install-location"
  ${Else}
    StrCpy $BiWorkRegInstallExe "$BiWorkRegInstallLocation\${BIWORK_APP_EXECUTABLE_FILENAME}"
    ${If} ${FileExists} "$BiWorkRegInstallExe"
      StrCpy $INSTDIR "$BiWorkRegInstallLocation"
      StrCpy $BiWorkRegistryInstallIsValid "1"
      !insertmacro BIWORK_LOG_EVENT "event=registry-heal phase=valid-install-location instDir=$INSTDIR uninstallString=$BiWorkRegUninstallString"
    ${Else}
      !insertmacro BIWORK_LOG_EVENT "event=registry-heal phase=stale-install-location installLocation=$BiWorkRegInstallLocation uninstallString=$BiWorkRegUninstallString"
      !insertmacro BIWORK_CLEAR_INSTALL_REGISTRY "stale-install-location"
    ${EndIf}
  ${EndIf}
!macroend

!macro BIWORK_LOG_UNINSTALL_RESULT _ROOT_KEY _HAD_ERRORS
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = '$BiWorkSessionId'; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = 'uninstall-result'; root = '${_ROOT_KEY}'; launchErrors = '${_HAD_ERRORS}'; exitCode = '$R0' }; \
    Add-Content -LiteralPath $$log -Encoding UTF8 -Value ($$payload | ConvertTo-Json -Compress -Depth 8) \
  }"`
  Pop $BiWorkUninstallLogResult
!macroend

!macro BIWORK_HANDLE_UNINSTALL_RESULT _ROOT_KEY _LABEL_PREFIX
  ${If} ${Errors}
    StrCpy $BiWorkUninstallHadErrors "1"
  ${Else}
    StrCpy $BiWorkUninstallHadErrors "0"
  ${EndIf}

  !insertmacro BIWORK_LOG_UNINSTALL_RESULT "${_ROOT_KEY}" "$BiWorkUninstallHadErrors"

  ${If} $BiWorkUninstallHadErrors == "1"
    DetailPrint `Uninstall was not successful. Not able to launch uninstaller!`
    Return
  ${EndIf}

  ${If} $R0 != 0
      DetailPrint `Uninstall was not successful. Uninstaller error code: $R0.`
      !insertmacro BIWORK_READ_LAST_INNER_FAILURE
      ${If} $BiWorkLockerList != ""
        StrCpy $BiWorkInnerFailureSummary "- Failure: previous uninstaller failed with exit code $R0$\r$\n- File or folder: $INSTDIR$\r$\n- Blocking process: $BiWorkLockerList"
      ${EndIf}
      !insertmacro BIWORK_LOG_EVENT "event=old-uninstaller-failed action=report exitCode=$R0 lockers=$BiWorkLockerList uninstallerDetail=$BiWorkInnerFailureSummary"
      ${If} $BiWorkInnerRootCode != ""
        !insertmacro BIWORK_FAIL_REPORTABLE_ROOTED_BILINGUAL_DIAGNOSTICS "$BiWorkInnerRootCode" ${BIWORK_E_OLD_UNINSTALL_FAILED} "old-uninstaller exitCode=$R0 lockers=$BiWorkLockerList uninstallerDetail=$BiWorkInnerFailureSummary" "${BIWORK_MSG_OLD_UNINSTALL_FAILED_EN}" "${BIWORK_MSG_OLD_UNINSTALL_FAILED_ZH}" "${BIWORK_MSG_OLD_UNINSTALL_ACTION_EN}" "${BIWORK_MSG_OLD_UNINSTALL_ACTION_ZH}" "$BiWorkInnerFailureSummary" "$BiWorkInnerFailureSummary"
      ${Else}
        !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL_DIAGNOSTICS ${BIWORK_E_OLD_UNINSTALL_FAILED} "old-uninstaller exitCode=$R0 lockers=$BiWorkLockerList uninstallerDetail=$BiWorkInnerFailureSummary" "${BIWORK_MSG_OLD_UNINSTALL_FAILED_EN}" "${BIWORK_MSG_OLD_UNINSTALL_FAILED_ZH}" "${BIWORK_MSG_OLD_UNINSTALL_ACTION_EN}" "${BIWORK_MSG_OLD_UNINSTALL_ACTION_ZH}" "$BiWorkInnerFailureSummary" "$BiWorkInnerFailureSummary"
      ${EndIf}
  ${EndIf}
!macroend

!macro customInit
  !insertmacro BIWORK_HEAL_INSTALL_REGISTRY
  ${If} $BiWorkRegistryInstallIsValid == "1"
    !insertmacro BIWORK_REPAIR_INSTALLED_UNINSTALLER
  ${EndIf}
!macroend

!macro customUnInstallCheck
  !insertmacro BIWORK_HANDLE_UNINSTALL_RESULT "SHELL_CONTEXT" "shctx"
!macroend

!macro customUnInstallCheckCurrentUser
  !insertmacro BIWORK_HANDLE_UNINSTALL_RESULT "HKEY_CURRENT_USER" "hkcu"
!macroend

!endif
