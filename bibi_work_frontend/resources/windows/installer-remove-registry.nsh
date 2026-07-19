!ifndef BIWORK_INSTALLER_REMOVE_REGISTRY_NSH
!define BIWORK_INSTALLER_REMOVE_REGISTRY_NSH

!macro BIWORK_CLEAR_INSTALL_REGISTRY _REASON
  DeleteRegKey SHCTX "${UNINSTALL_REGISTRY_KEY}"
  DeleteRegKey SHCTX "${INSTALL_REGISTRY_KEY}"
  !insertmacro BIWORK_LOG_EVENT "event=registry-clear reason=${_REASON} uninstallKey=${UNINSTALL_REGISTRY_KEY} installKey=${INSTALL_REGISTRY_KEY}"
!macroend

!macro BIWORK_LOG_ATOMIC_REMOVE_FAILURE
  Push $9
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$failed = '$BiWorkAtomicFailedPath'; \
    $$instDir = '$INSTDIR'; \
    $$oldInstallDir = '$BiWorkAtomicStagingDir'; \
    $$relative = $$failed; \
    if ($$failed.StartsWith($$instDir, [System.StringComparison]::CurrentCultureIgnoreCase)) { $$relative = $$failed.Substring($$instDir.Length).TrimStart('\') }; \
    $$tempCandidate = if ($$relative -and $$relative -ne $$failed) { Join-Path $$oldInstallDir $$relative } else { '' }; \
    $$kind = if ($$tempCandidate.Length -ge 260) { 'likely-long-path' } else { 'unknown' }; \
    $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = '$BiWorkSessionId'; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = 'remove-atomic-failed'; kind = $$kind; pathLength = $$failed.Length; tempCandidateLength = $$tempCandidate.Length; atomicFailedPath = $$failed; tempCandidate = $$tempCandidate }; \
    Add-Content -LiteralPath $$log -Encoding UTF8 -Value ($$payload | ConvertTo-Json -Compress -Depth 8) \
  }"`
  Pop $9
  Pop $9
!macroend

!macro BIWORK_LOG_REMOVE_FAILURE_JSON _PHASE _FATAL _FAILED_PATH _EXTRA_FIELDS
  !insertmacro BIWORK_LOG_JSON_EVENT "failure" "$$lockerText = '$BiWorkLockerList'; $$processes = @(); if ($$lockerText -and $$lockerText -notlike 'Windows did not identify*' -and $$lockerText -ne 'unknown process') { $$processes = @($$lockerText -split ',\s*' | Where-Object { $$_ } | ForEach-Object { if ($$_ -match '^(.*)\(([0-9]+)\)$$') { [ordered]@{ name = $$Matches[1]; pid = [int]$$Matches[2] } } else { [ordered]@{ name = $$_; pid = $$null } } }) }; $$payload.code = '${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED}'; $$payload.phase = '${_PHASE}'; $$payload.failedPath = '${_FAILED_PATH}'; $$payload.blockingProcesses = @($$processes); if ($$lockerText -like 'BiWork installer(*)') { $$payload.fallbackReason = 'installer-self-lock'; $$payload.message = 'The installer process is using the install directory as its current output directory.' } elseif ($$processes.Count -eq 0) { $$payload.fallbackReason = 'restart-manager-no-process'; $$payload.message = 'Windows did not identify a specific locking process. Close terminals, editors, and file managers opened in the install folder.' } else { $$payload.fallbackReason = ''; $$payload.message = '' }; $$payload.fatal = ('${_FATAL}' -eq '1'); ${_EXTRA_FIELDS}"
!macroend

!macro BIWORK_REMOVE_INSTALL_DIR
  StrCpy $BiWorkRemoveResidueCount "0"
  ${If} $BiWorkRemoveResidueRoot == ""
    StrCpy $BiWorkRemoveResidueRoot "$INSTDIR"
  ${EndIf}
  StrCpy $BiWorkRemoveFirstFailedPath ""
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'Continue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$path = [System.IO.Path]::GetFullPath('$BiWorkRemoveResidueRoot'); \
    $$firstFailedFile = '$PLUGINSDIR\biwork-remove-first-failed.txt'; \
    Set-Content -LiteralPath $$firstFailedFile -Encoding UTF8 -NoNewline -Value ''; \
    function Write-InstallerLog($$message) { $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = '$BiWorkSessionId'; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = 'remove-log'; message = $$message }; if ($$message -match '(^|\s)event=([^\s]+)') { $$payload.event = $$Matches[2] }; Add-Content -LiteralPath $$log -Encoding UTF8 -Value ($$payload | ConvertTo-Json -Compress -Depth 8) } \
    function Convert-LongPath($$itemPath) { if ($$itemPath.StartsWith('\\')) { return '\\?\UNC\' + $$itemPath.TrimStart('\') } return '\\?\' + $$itemPath } \
    function Remove-WithRetries($$item, $$isDir) { \
      $$delays = @(200,500,1000); \
      for ($$i = 0; $$i -lt $$delays.Count; $$i++) { \
        try { \
          if ($$isDir) { [System.IO.Directory]::Delete((Convert-LongPath $$item), $$false) } else { [System.IO.File]::Delete((Convert-LongPath $$item)) } \
          return $$true \
        } catch { \
          if ($$i -lt $$delays.Count - 1) { Start-Sleep -Milliseconds $$delays[$$i] } else { Write-InstallerLog ('event=remove-resilient-leftover path=' + $$item + ' attempts=3 error=' + $$_.Exception.GetType().FullName + ': ' + $$_.Exception.Message); return $$false } \
        } \
      } \
      return $$false \
    } \
    try { \
      if (-not (Test-Path -LiteralPath $$path)) { Write-InstallerLog ('remove-longpath result=0 instDir=' + $$path); exit 0 } \
      $$failed = New-Object System.Collections.Generic.List[string]; \
      foreach ($$file in @(Get-ChildItem -LiteralPath $$path -Force -Recurse -File -ErrorAction SilentlyContinue | Sort-Object FullName -Descending)) { if (-not (Remove-WithRetries $$file.FullName $$false)) { $$failed.Add($$file.FullName) } } \
      foreach ($$dir in @(Get-ChildItem -LiteralPath $$path -Force -Recurse -Directory -ErrorAction SilentlyContinue | Sort-Object FullName -Descending)) { if (-not (Remove-WithRetries $$dir.FullName $$true)) { $$failed.Add($$dir.FullName) } } \
      if (-not (Remove-WithRetries $$path $$true)) { $$failed.Add($$path) } \
      Write-InstallerLog ('event=remove-resilient-summary failedCount=' + $$failed.Count + ' root=' + $$path); \
      if ($$failed.Count -gt 0) { Set-Content -LiteralPath $$firstFailedFile -Encoding UTF8 -NoNewline -Value $$failed[0]; exit $$failed.Count } \
      Write-InstallerLog ('remove-longpath result=0 instDir=' + $$path); \
      exit 0 \
    } catch { \
      Write-InstallerLog ('remove-longpath result=1 instDir=' + $$path + ' error=' + $$_.Exception.GetType().FullName + ': ' + $$_.Exception.Message); \
      exit 1 \
    } \
  }"`
  Pop $BiWorkRemoveDirResult

  ClearErrors
  SetDetailsPrint none
  FileOpen $BiWorkRemoveFirstFailedFile "$PLUGINSDIR\biwork-remove-first-failed.txt" r
  ${IfNot} ${Errors}
    FileRead $BiWorkRemoveFirstFailedFile $BiWorkRemoveFirstFailedPath
    FileClose $BiWorkRemoveFirstFailedFile
  ${EndIf}
  SetDetailsPrint lastused

  ${If} $BiWorkRemoveDirResult == "error"
    !insertmacro BIWORK_LOG_EVENT "event=remove-longpath fallback=RMDir reason=no-powershell root=$INSTDIR"
    RMDir /r "$BiWorkRemoveResidueRoot"
    ${If} ${FileExists} "$BiWorkRemoveResidueRoot\*.*"
      StrCpy $BiWorkRemoveDirResult "1"
    ${Else}
      StrCpy $BiWorkRemoveDirResult "0"
    ${EndIf}
  ${EndIf}

  ${If} $BiWorkRemoveDirResult != 0
    StrCpy $BiWorkRemoveResidueCount $BiWorkRemoveDirResult
  ${EndIf}
!macroend

!macro customRemoveFiles
  !insertmacro BIWORK_LOG_EVENT "remove-start instDir=$INSTDIR"
  Var /GLOBAL BiWorkRemoveDirResult
  Var /GLOBAL BiWorkAtomicFailedPath
  Var /GLOBAL BiWorkAtomicRemoveSucceeded
  Var /GLOBAL BiWorkAtomicStagingDir
  Var /GLOBAL BiWorkRemoveResidueCount
  Var /GLOBAL BiWorkRemoveResidueRoot
  Var /GLOBAL BiWorkRemoveFirstFailedPath
  Var /GLOBAL BiWorkRemoveFirstFailedFile
  StrCpy $BiWorkAtomicFailedPath ""
  StrCpy $BiWorkAtomicRemoveSucceeded "0"
  StrCpy $BiWorkAtomicStagingDir ""
  StrCpy $BiWorkRemoveResidueCount "0"
  StrCpy $BiWorkRemoveResidueRoot "$INSTDIR"
  StrCpy $BiWorkRemoveFirstFailedPath ""

  SetOutPath $TEMP
  StrCpy $BiWorkCurrentOutDir "$TEMP"

  ${if} ${isUpdated}
    StrCpy $BiWorkAtomicStagingDir "$INSTDIR.__old"
    ${If} ${FileExists} "$BiWorkAtomicStagingDir\*.*"
      StrCpy $BiWorkRemoveResidueRoot "$BiWorkAtomicStagingDir"
      !insertmacro BIWORK_LOG_EVENT "remove-stale-staging start root=$BiWorkRemoveResidueRoot"
      !insertmacro BIWORK_REMOVE_INSTALL_DIR
      StrCpy $BiWorkRemoveResidueRoot "$INSTDIR"
    ${EndIf}

    biwork_retry_atomic_rename:
      ClearErrors
      Rename "$INSTDIR" "$BiWorkAtomicStagingDir"
    ${if} ${Errors}
      DetailPrint "Atomic update cleanup failed before replacing previous installation: $INSTDIR"
      StrCpy $BiWorkAtomicFailedPath "$INSTDIR"
      !insertmacro BIWORK_LOG_ATOMIC_REMOVE_FAILURE
      !insertmacro BIWORK_CAPTURE_FAILED_PATH_LOCKERS "$BiWorkAtomicFailedPath"
      ${IfNot} ${Silent}
        !insertmacro BIWORK_PROMPT_FAILED_PATH_LOCKERS "$BiWorkAtomicFailedPath" "atomic-failed" biwork_retry_atomic_rename biwork_cancel_atomic_rename biwork_continue_atomic_failed
        biwork_cancel_atomic_rename:
      ${EndIf}
      biwork_continue_atomic_failed:
      !insertmacro BIWORK_LOG_REMOVE_FAILURE_JSON "atomic-failed" "1" "$BiWorkAtomicFailedPath" "$$payload.atomicFailedPath = '$BiWorkAtomicFailedPath'"
      !insertmacro BIWORK_LOG_EVENT "code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=atomic-failed fatal=1 degraded=none firstFailed=$BiWorkAtomicFailedPath atomicFailedPath=$BiWorkAtomicFailedPath"
      !insertmacro BIWORK_CLEAR_INSTALL_REGISTRY "remove-failed-before-quit"
      !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL ${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} "event=session-end result=fail code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=atomic-failed fatal=1 firstFailed=$BiWorkAtomicFailedPath lockers=$BiWorkLockerList" "${BIWORK_MSG_REPLACE_LOCKED_EN}" "${BIWORK_MSG_REPLACE_LOCKED_ZH}" "${BIWORK_MSG_CLOSE_SHOWN_FILE_ACTION_EN}" "${BIWORK_MSG_CLOSE_SHOWN_FILE_ACTION_ZH}"
    ${else}
      !insertmacro BIWORK_LOG_EVENT "remove-atomic result=0 staging=$BiWorkAtomicStagingDir"
      StrCpy $BiWorkAtomicRemoveSucceeded "1"
      StrCpy $BiWorkRemoveResidueRoot "$BiWorkAtomicStagingDir"
    ${endif}
  ${endif}

  biwork_retry_remove_install_dir:
    !insertmacro BIWORK_REMOVE_INSTALL_DIR
  ${if} $BiWorkRemoveDirResult != 0
    !insertmacro BIWORK_CAPTURE_FAILED_PATH_LOCKERS "$BiWorkRemoveFirstFailedPath"
    ${if} $BiWorkAtomicRemoveSucceeded == "1"
      ${IfNot} ${Silent}
        !insertmacro BIWORK_PROMPT_FAILED_PATH_LOCKERS "$BiWorkRemoveFirstFailedPath" "residual-delete-failed" biwork_retry_remove_install_dir biwork_cancel_remove_after_rm biwork_continue_after_rm
        biwork_cancel_remove_after_rm:
          !insertmacro BIWORK_LOG_REMOVE_FAILURE_JSON "residual-delete-failed" "1" "$BiWorkRemoveFirstFailedPath" "$$payload.residueRoot = '$BiWorkRemoveResidueRoot'; $$payload.failedCount = '$BiWorkRemoveResidueCount'; $$payload.removeDirResult = '$BiWorkRemoveDirResult'; $$payload.atomicSucceeded = ('$BiWorkAtomicRemoveSucceeded' -eq '1')"
          !insertmacro BIWORK_LOG_EVENT "code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=residual-delete-failed userAction=cancel fatal=1 residueRoot=$BiWorkRemoveResidueRoot failedCount=$BiWorkRemoveResidueCount firstFailed=$BiWorkRemoveFirstFailedPath removeDirResult=$BiWorkRemoveDirResult removeResidueCount=$BiWorkRemoveResidueCount atomicFailedPath=$BiWorkAtomicFailedPath atomicSucceeded=$BiWorkAtomicRemoveSucceeded"
          !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL ${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} "event=session-end result=fail code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=residual-delete-failed userAction=cancel fatal=1 firstFailed=$BiWorkRemoveFirstFailedPath lockers=$BiWorkLockerList" "${BIWORK_MSG_PREVIOUS_FILE_OPEN_EN}" "${BIWORK_MSG_PREVIOUS_FILE_OPEN_ZH}" "${BIWORK_MSG_CLOSE_SHOWN_FILE_ACTION_EN}" "${BIWORK_MSG_CLOSE_SHOWN_FILE_ACTION_ZH}"
      ${EndIf}
      biwork_continue_after_rm:
      DetailPrint `BiWork previous installation had locked residual files; continuing after atomic cleanup succeeded: $INSTDIR`
      !insertmacro BIWORK_LOG_EVENT "code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=residual-delete-failed degraded=continue fatal=0 residueRoot=$BiWorkRemoveResidueRoot failedCount=$BiWorkRemoveResidueCount firstFailed=$BiWorkRemoveFirstFailedPath removeDirResult=$BiWorkRemoveDirResult removeResidueCount=$BiWorkRemoveResidueCount atomicFailedPath=$BiWorkAtomicFailedPath atomicSucceeded=$BiWorkAtomicRemoveSucceeded"
    ${else}
      DetailPrint `Can't safely remove previous installation without atomic cleanup proof: $INSTDIR`
      ${IfNot} ${Silent}
        !insertmacro BIWORK_PROMPT_FAILED_PATH_LOCKERS "$BiWorkRemoveFirstFailedPath" "residual-delete-failed-no-atomic-proof" biwork_retry_remove_install_dir biwork_cancel_remove_no_atomic biwork_continue_remove_no_atomic
        biwork_cancel_remove_no_atomic:
      ${EndIf}
      biwork_continue_remove_no_atomic:
      !insertmacro BIWORK_LOG_REMOVE_FAILURE_JSON "residual-delete-failed-no-atomic-proof" "1" "$BiWorkRemoveFirstFailedPath" "$$payload.residueRoot = '$BiWorkRemoveResidueRoot'; $$payload.failedCount = '$BiWorkRemoveResidueCount'; $$payload.removeDirResult = '$BiWorkRemoveDirResult'; $$payload.atomicSucceeded = ('$BiWorkAtomicRemoveSucceeded' -eq '1')"
      !insertmacro BIWORK_LOG_EVENT "code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=residual-delete-failed-no-atomic-proof degraded=none fatal=1 residueRoot=$BiWorkRemoveResidueRoot failedCount=$BiWorkRemoveResidueCount firstFailed=$BiWorkRemoveFirstFailedPath removeDirResult=$BiWorkRemoveDirResult removeResidueCount=$BiWorkRemoveResidueCount atomicFailedPath=$BiWorkAtomicFailedPath atomicSucceeded=$BiWorkAtomicRemoveSucceeded"
      !insertmacro BIWORK_CLEAR_INSTALL_REGISTRY "remove-failed-before-quit"
      !insertmacro BIWORK_FAIL_REPORTABLE_BILINGUAL ${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} "event=session-end result=fail code=${BIWORK_E_INSTALL_DIR_REMOVE_OR_LOCKED} phase=residual-delete-failed-no-atomic-proof fatal=1 firstFailed=$BiWorkRemoveFirstFailedPath removeDirResult=$BiWorkRemoveDirResult lockers=$BiWorkLockerList" "${BIWORK_MSG_REMOVE_PREVIOUS_DIR_EN}" "${BIWORK_MSG_REMOVE_PREVIOUS_DIR_ZH}" "${BIWORK_MSG_CLOSE_INSTALL_DIR_ACTION_EN}" "${BIWORK_MSG_CLOSE_INSTALL_DIR_ACTION_ZH}"
    ${endif}
  ${else}
    !insertmacro BIWORK_LOG_EVENT "remove-final errors=0 instDir=$INSTDIR removeDirResult=$BiWorkRemoveDirResult removeResidueCount=$BiWorkRemoveResidueCount removeResidueRoot=$BiWorkRemoveResidueRoot atomicFailedPath=$BiWorkAtomicFailedPath atomicSucceeded=$BiWorkAtomicRemoveSucceeded"
  ${endif}
!macroend

!macro customUnInit
  !insertmacro BIWORK_LOG_EVENT "uninit instDir=$INSTDIR"
!macroend

!macro customUnInstall
  !insertmacro BIWORK_LOG_EVENT "uninstall-section start instDir=$INSTDIR"
!macroend

!endif
