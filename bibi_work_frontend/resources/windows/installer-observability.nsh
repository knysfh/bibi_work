!ifndef BIWORK_INSTALLER_OBSERVABILITY_NSH
!define BIWORK_INSTALLER_OBSERVABILITY_NSH

!define BIWORK_APP_EXECUTABLE_FILENAME "BiWork.exe"
!define BIWORK_FALLBACK_LOG "biwork-installer-${VERSION}-fallback-log.jsonl"

!pragma warning disable 6001
Var /GLOBAL BiWorkSessionId
Var /GLOBAL BiWorkIsUpdated
Var /GLOBAL BiWorkSessionLogResult
Var /GLOBAL BiWorkSessionLogPath

!macro BIWORK_SESSION_HEADER
  !insertmacro BIWORK_SLOG "event=header arch=${BIWORK_TARGET_ARCH} updated=$BiWorkIsUpdated instDir=$INSTDIR version=${VERSION} log=$BiWorkSessionLogPath detail=customHeader"
!macroend

!macro BIWORK_SLOG _MESSAGE
  Push $9
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$session = '$BiWorkSessionId'; \
    if (-not $$session) { $$session = 'uninitialized' }; \
    $$message = '${_MESSAGE}'; \
    $$event = 'log'; \
    if ($$message -match '(^|\s)event=([^\s]+)') { $$event = $$Matches[2] } else { $$first = @($$message -split '\s+', 2)[0]; if ($$first -and $$first -notmatch '=') { $$event = $$first } }; \
    $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = $$session; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = $$event; message = $$message }; \
    $$json = $$payload | ConvertTo-Json -Compress -Depth 8; \
    Add-Content -LiteralPath $$log -Encoding UTF8 -Value $$json \
  }"`
  Pop $9
  Pop $9
!macroend

!macro BIWORK_LOG_EVENT _MESSAGE
  Push $9
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$session = '$BiWorkSessionId'; \
    if (-not $$session) { $$session = 'uninitialized' }; \
    $$message = '${_MESSAGE}'; \
    $$event = 'log'; \
    if ($$message -match '(^|\s)event=([^\s]+)') { $$event = $$Matches[2] } else { $$first = @($$message -split '\s+', 2)[0]; if ($$first -and $$first -notmatch '=') { $$event = $$first } }; \
    $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = $$session; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = $$event; message = $$message }; \
    $$json = $$payload | ConvertTo-Json -Compress -Depth 8; \
    Add-Content -LiteralPath $$log -Encoding UTF8 -Value $$json \
  }"`
  Pop $9
  Pop $9
!macroend

!macro BIWORK_LOG_JSON_EVENT _EVENT _JSON_FIELDS
  Push $9
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "& { \
    $$ErrorActionPreference = 'SilentlyContinue'; \
    $$log = '$BiWorkSessionLogPath'; \
    if (-not $$log) { $$log = Join-Path $$env:TEMP '${BIWORK_FALLBACK_LOG}' }; \
    $$session = '$BiWorkSessionId'; \
    if (-not $$session) { $$session = 'uninitialized' }; \
    $$payload = [ordered]@{ schemaVersion = 1; ts = (Get-Date -Format o); session = $$session; version = '${VERSION}'; arch = '${BIWORK_TARGET_ARCH}'; updated = ('$BiWorkIsUpdated' -eq '1'); instDir = '$INSTDIR'; event = '${_EVENT}' }; \
    ${_JSON_FIELDS}; \
    $$json = $$payload | ConvertTo-Json -Compress -Depth 8; \
    Add-Content -LiteralPath $$log -Encoding UTF8 -Value $$json \
  }"`
  Pop $9
  Pop $9
!macroend

!macro BIWORK_SESSION_BEGIN
  ${GetParameters} $R9
  ClearErrors
  ${GetOptions} $R9 "--installer-log=" $R8
  ${IfNot} ${Errors}
    StrCpy $BiWorkSessionLogPath $R8
  ${EndIf}
  ClearErrors
  ${GetOptions} $R9 "--installer-session=" $R8
  ${IfNot} ${Errors}
    StrCpy $BiWorkSessionId $R8
  ${EndIf}

  ${If} $BiWorkSessionLogPath == ""
    nsExec::ExecToStack `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "$$id = '$BiWorkSessionId'; if (-not $$id) { $$id = [guid]::NewGuid().ToString('N').Substring(0,12) }; $$stamp = Get-Date -Format 'yyyyMMdd'; $$name = 'biwork-installer-${VERSION}-' + $$stamp + '-log.jsonl'; $$log = Join-Path $$env:TEMP $$name; [Console]::Out.Write($$id + '|' + $$log)"`
    Pop $BiWorkSessionLogResult
    Pop $BiWorkSessionLogResult
    StrCpy $BiWorkSessionId $BiWorkSessionLogResult 12
    StrCpy $BiWorkSessionLogPath $BiWorkSessionLogResult 1024 13
  ${ElseIf} $BiWorkSessionId == ""
    nsExec::ExecToStack `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "[Console]::Out.Write([guid]::NewGuid().ToString('N').Substring(0,12))"`
    Pop $BiWorkSessionLogResult
    Pop $BiWorkSessionLogResult
    StrCpy $BiWorkSessionId $BiWorkSessionLogResult
  ${EndIf}

  ClearErrors
  ${GetOptions} $R9 "--updated" $R8
  StrCpy $BiWorkIsUpdated "0"
  ${IfNot} ${Errors}
    StrCpy $BiWorkIsUpdated "1"
  ${EndIf}

  !insertmacro BIWORK_SLOG "event=session-begin detail=preInit"
!macroend

!macro BIWORK_LOG_EXTRACT_RESULT _METHOD
  ${IfNot} ${FileExists} "$INSTDIR\BiWork.exe"
    !insertmacro BIWORK_FAIL_UX \
      "${BIWORK_E_EXTRACT_FAILED}" \
      "event=extract result=fail method=${_METHOD} missing=BiWork.exe" \
      "${BIWORK_MSG_EXTRACT_FAILED_ZH}" \
      "${BIWORK_MSG_EXTRACT_FAILED_EN}" \
      "${BIWORK_MSG_EXTRACT_FAILED_ACTION_ZH}" \
      "${BIWORK_MSG_EXTRACT_FAILED_ACTION_EN}" \
      "extract result=fail method=${_METHOD} missing=BiWork.exe instDir=$INSTDIR" \
      "extract result=fail method=${_METHOD} missing=BiWork.exe instDir=$INSTDIR"
  ${Else}
    !insertmacro BIWORK_SLOG "event=extract result=ok method=${_METHOD} detail=customFiles_${BIWORK_TARGET_ARCH}"
  ${EndIf}
!macroend

!macro BIWORK_SESSION_SUCCESS
  !insertmacro BIWORK_SLOG "event=session-end result=success detail=customInstall"
!macroend

!endif
