; PC Agent installer hooks (Tauri NSIS). Per-user install: nothing here needs administrator rights.

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove this account's daily report task before the files go. Fixed argument, journaled,
  ; refused when elevated; an absent task is not an error.
  ${If} ${FileExists} "$INSTDIR\pcagent-gui.exe"
    ExecWait '"$INSTDIR\pcagent-gui.exe" --remove-scheduled-report'
  ${EndIf}
!macroend
