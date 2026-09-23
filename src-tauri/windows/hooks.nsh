; Uninstaller hooks for the NSIS installer. See `src/login.rs`.
;
; Tauri's uninstaller already deletes the `Run` value that opens Chief at
; login, but not the Task Manager `StartupApproved` value the autostart plugin
; writes beside it. Both are named after the product. Neither is touched on an
; update, which runs the uninstaller with /UPDATE: an upgrade must not turn the
; setting off.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"
  ${EndIf}
!macroend
