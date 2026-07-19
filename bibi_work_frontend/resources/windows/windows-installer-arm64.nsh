; ARM64 architecture entry for the NSIS installer.

!include "x64.nsh"

!define BIWORK_TARGET_ARCH "arm64"
!define BIWORK_RUNTIME_KEY "win32-arm64"
!define BIWORK_EXTRACT_METHOD "zip"

!addincludedir "${PROJECT_DIR}\resources\windows"
!include "installer-common.nsh"

!macro customHeader
  !insertmacro BIWORK_INSTALLER_CUSTOM_HEADER
!macroend

!macro preInit
  !insertmacro BIWORK_INSTALLER_PREINIT
!macroend

!macro customFiles_arm64
  !insertmacro BIWORK_LOG_EXTRACT_RESULT "zip"
!macroend

Function .onVerifyInstDir
  ${IfNot} ${IsNativeARM64}
    !insertmacro BIWORK_FAIL_UX \
      "${BIWORK_E_ARCH_MISMATCH}" \
      "target=arm64 actual=non-arm64" \
      "${BIWORK_MSG_ARCH_MISMATCH_ZH}" \
      "${BIWORK_MSG_ARCH_MISMATCH_EN}" \
      "${BIWORK_MSG_ARCH_MISMATCH_ACTION_ZH}" \
      "${BIWORK_MSG_ARCH_MISMATCH_ACTION_EN}" \
      "target=arm64 actual=non-arm64" \
      "target=arm64 actual=non-arm64"
  ${EndIf}
FunctionEnd
