!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\Windows NT\CurrentVersion" "CurrentBuildNumber"
  ${If} $0 < 22000
    MessageBox MB_OK|MB_ICONSTOP "Loofah requires Windows 11 or later."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Updates preserve the opt-in CLI so startup can refresh it with the new build.
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-integrations' $0
    ${If} $0 <> 0
      DetailPrint "Some Loofah integrations were kept because their files were changed or in use."
    ${EndIf}
  ${EndIf}
!macroend
