; Hivetty additions to the Tauri NSIS installer (per-machine install: C:\Program Files\Hivetty,
; runs elevated).
;
; Install: put $INSTDIR\bin (holding only hivetty.exe) on the machine PATH.
; Real uninstall (not the uninstall step of an update): stop the detached session host, take
; bin off PATH, and remove the runtime copies of tmux.exe / hivetty-host.exe.
; User settings and sessions in %APPDATA%\Hivetty are left alone.

; In-app updates start this installer with /WAITPID=<pid of the running Hivetty>, then end that
; process. Wait (up to 20 s) until it's really gone, then until nothing else (antivirus scanners,
; seen: McAfee, ~30 s) still holds the old exe: otherwise the installer's "is it running?" check
; (Restart Manager) aborts with "insufficient permissions to close Hivetty".
;
; Some scanners (seen: McAfee Framework Host, for minutes) keep the old exe open long after
; Hivetty has exited. Tauri's check then tries to shut the scanner down, can't, and aborts with
; "Failed to kill Hivetty". So once Hivetty itself is known to be gone and the file is still held,
; rename it aside (Windows allows renaming a file that's open). The new exe is installed under
; the normal name; the old copy is deleted now if possible, otherwise on the next update or reboot.
!macro NSIS_HOOK_PREINSTALL
  ; leftovers from an earlier update
  Delete "$INSTDIR\${MAINBINARYNAME}.exe.old-*"
  ; $8 = "1" once the Hivetty that started this update has exited
  StrCpy $8 "0"
  ClearErrors
  ${GetOptions} $CMDLINE "/WAITPID=" $R9
  ${IfNot} ${Errors}
    ; SYNCHRONIZE = 0x00100000
    System::Call 'kernel32::OpenProcess(i 0x00100000, i 0, i R9) p .R8'
    ${If} $R8 = 0
      StrCpy $8 "1" ; already gone
    ${Else}
      System::Call 'kernel32::WaitForSingleObject(p R8, i 20000) i .R7'
      ${If} $R7 = 0 ; WAIT_OBJECT_0
        StrCpy $8 "1"
      ${EndIf}
      System::Call 'kernel32::CloseHandle(p R8)'
    ${EndIf}
  ${EndIf}
  ${If} $UpdateMode = 1
    DetailPrint "Waiting for Hivetty's files to be released..."
    System::Call '*(&w520 "$INSTDIR\${MAINBINARYNAME}.exe") p .R4'
    System::Call '*(p R4) p .R3'
    StrCpy $R6 0
    hivetty_rm_loop:
      System::Call 'RSTRTMGR::RmStartSession(*i .R1, i 0, w .R2) i .R0'
      ${If} $R0 = 0
        System::Call 'RSTRTMGR::RmRegisterResources(i R1, i 1, p R3, i 0, p 0, i 0, p 0) i .R0'
        System::Call 'RSTRTMGR::RmGetList(i R1, *i .R5, *i 0, p 0, *i .R7) i .R0'
        System::Call 'RSTRTMGR::RmEndSession(i R1)'
        ; 234 = ERROR_MORE_DATA: something still has the file open
        ${If} $R0 = 234
        ${AndIf} $R6 < 60
          Sleep 500
          IntOp $R6 $R6 + 1
          Goto hivetty_rm_loop
        ${EndIf}
      ${EndIf}
    System::Free $R3
    System::Free $R4
    ; still held after ~30 s, and not by Hivetty: move it out of the way
    ${If} $R0 = 234
    ${AndIf} $8 == "1"
      System::Call 'kernel32::GetTickCount() i .R5'
      ClearErrors
      Rename "$INSTDIR\${MAINBINARYNAME}.exe" "$INSTDIR\${MAINBINARYNAME}.exe.old-$R5"
      ${IfNot} ${Errors}
        DetailPrint "Another program (an antivirus scan?) still had Hivetty open; replacing it anyway."
        Delete /REBOOTOK "$INSTDIR\${MAINBINARYNAME}.exe.old-$R5"
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" setup-path --machine --add "$INSTDIR\bin"'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} ${FileExists} "$INSTDIR\bin\hivetty.exe"
      nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" host stop'
      Pop $0
      ; agent hooks point at this hivetty.exe; drop them so agents don't call a missing program
      nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" hooks remove'
      Pop $0
      nsExec::ExecToLog '"$INSTDIR\bin\hivetty.exe" setup-path --machine --remove "$INSTDIR\bin"'
      Pop $0
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; the installing user's own %LOCALAPPDATA% ($LOCALAPPDATA means all-users here)
    ReadEnvStr $0 LOCALAPPDATA
    ${If} $0 != ""
      RMDir /r /REBOOTOK "$0\Hivetty\runtime"
      RMDir "$0\Hivetty"
    ${EndIf}
  ${EndIf}
!macroend
