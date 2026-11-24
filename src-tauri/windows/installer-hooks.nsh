; Hivetty additions to the Tauri NSIS installer (per-user install).
;
; Install: put $INSTDIR\bin (holding only hivetty.exe) on the user's PATH.
; Real uninstall (not the uninstall step of an update): stop the detached session host, take
; bin off PATH, and remove the runtime copies of tmux.exe / hivetty-host.exe.
; User settings and sessions in %APPDATA%\Hivetty are left alone.

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" setup-path --add "$INSTDIR\bin"'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} ${FileExists} "$INSTDIR\bin\hivetty.exe"
      nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" host stop'
      Pop $0
      nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" setup-path --remove "$INSTDIR\bin"'
      Pop $0
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    RMDir /r /REBOOTOK "$LOCALAPPDATA\Hivetty\runtime"
    RMDir "$INSTDIR\bin"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
