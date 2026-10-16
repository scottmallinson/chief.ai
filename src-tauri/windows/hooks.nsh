; Hooks for the NSIS installer. See `src/login.rs` for the login entry.
;
; Chief's model engine is a separate program, llama-server.exe, installed beside
; chief.exe, and it loads ggml-base.dll and the other libraries from that
; folder. Tauri's installer only closes chief.exe, so an engine left running
; (Chief force-quit, or ended from Task Manager) holds those files open and the
; install stops with "Error opening file for writing: ...\ggml-base.dll". Ending
; it first costs nothing: it holds no state, and Chief starts it again on
; demand. Stopping a process that is not running is a harmless no-op.
;
; Tauri's uninstaller already deletes the `Run` value that opens Chief at
; login, but not the Task Manager `StartupApproved` value the autostart plugin
; writes beside it. Both are named after the product. Neither is touched on an
; update, which runs the uninstaller with /UPDATE: an upgrade must not turn the
; setting off.

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /T /IM llama-server.exe'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /T /IM llama-server.exe'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"
  ${EndIf}
!macroend
