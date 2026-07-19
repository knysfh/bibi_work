; x64 architecture entry for the NSIS installer.

!include "x64.nsh"

!define BIWORK_TARGET_ARCH "x64"
!define BIWORK_RUNTIME_KEY "win32-x64"
!define BIWORK_EXTRACT_METHOD "7z"

!addincludedir "${PROJECT_DIR}\resources\windows"
!include "installer-common.nsh"

!macro customHeader
  !insertmacro BIWORK_INSTALLER_CUSTOM_HEADER
!macroend

!macro preInit
  !insertmacro BIWORK_INSTALLER_PREINIT
!macroend

!macro customFiles_x64
  !insertmacro BIWORK_LOG_EXTRACT_RESULT "7z"
!macroend

Function .onVerifyInstDir
  ${IfNot} ${RunningX64}
    !insertmacro BIWORK_FAIL_UX \
      "${BIWORK_E_ARCH_MISMATCH}" \
      "target=x64 actual=x86" \
      "${BIWORK_MSG_ARCH_MISMATCH_ZH}" \
      "${BIWORK_MSG_ARCH_MISMATCH_EN}" \
      "${BIWORK_MSG_ARCH_MISMATCH_ACTION_ZH}" \
      "${BIWORK_MSG_ARCH_MISMATCH_ACTION_EN}" \
      "target=x64 actual=x86" \
      "target=x64 actual=x86"
  ${EndIf}

  ${If} ${IsNativeARM64}
    !insertmacro BIWORK_FAIL_UX \
      "${BIWORK_E_ARCH_MISMATCH}" \
      "target=x64 actual=arm64" \
      "${BIWORK_MSG_ARCH_MISMATCH_ZH}" \
      "${BIWORK_MSG_ARCH_MISMATCH_EN}" \
      "${BIWORK_MSG_ARCH_MISMATCH_ACTION_ZH}" \
      "${BIWORK_MSG_ARCH_MISMATCH_ACTION_EN}" \
      "target=x64 actual=arm64" \
      "target=x64 actual=arm64"
  ${EndIf}
FunctionEnd
