; Custom NSIS hooks for the Tauri v2 installer template, which only expands the
; NSIS_HOOK_{PRE,POST}{INSTALL,UNINSTALL} macros (wired via
; bundle.windows.nsis.installerHooks in tauri.conf.json).
;
; - Add/remove $INSTDIR to the user PATH so `fleet` resolves in new terminals.
; - Move a running fleet.exe sidecar out of the way before it is overwritten.
;   Claude turns are hosted by `fleet.exe fleet-claude-host` and every turn runs
;   `fleet.exe mcp`; the template only kills the main app, and overwriting a
;   running exe fails with "Error opening file for writing". Windows allows
;   renaming a running exe within its volume, so the old process keeps running
;   from the renamed file and the new fleet.exe is written beside it.
;
; Keep this file ASCII: makensis reads it in the build machine's codepage.

!define FLEET_SIDECAR "fleet.exe"

; PowerShell reads the directory from FLEET_INSTDIR instead of having it spliced
; into the command line, so no quoting of the path is needed. The registry API
; (not [Environment]) keeps PATH as REG_EXPAND_SZ and leaves %VAR% entries alone.
!macro FLEET_EDIT_USER_PATH action
  System::Call 'kernel32::SetEnvironmentVariable(t "FLEET_INSTDIR", t "$INSTDIR")'
  nsExec::ExecToLog `powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command "\
    $$d = $$env:FLEET_INSTDIR; \
    $$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); \
    $$p = [string]$$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
    $$parts = @($$p -split ';' | Where-Object { $$_ -ne '' }); \
    if ('${action}' -eq 'add') { if ($$parts -notcontains $$d) { $$parts += $$d } } \
    else { $$parts = @($$parts | Where-Object { $$_ -ne $$d }) }; \
    $$k.SetValue('Path', ($$parts -join ';'), 'ExpandString')"`
  Pop $0
  ; Broadcast so Explorer (and shells it starts) pick up the new PATH
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_PREINSTALL
  Push $R0
  ; Leftovers from earlier upgrades; ones whose process still runs stay locked
  ; and are retried on the next install.
  Delete "$INSTDIR\${FLEET_SIDECAR}.old-*"
  ${If} ${FileExists} "$INSTDIR\${FLEET_SIDECAR}"
    StrCpy $R0 0
    ${While} ${FileExists} "$INSTDIR\${FLEET_SIDECAR}.old-$R0"
      IntOp $R0 $R0 + 1
    ${EndWhile}
    ClearErrors
    Rename "$INSTDIR\${FLEET_SIDECAR}" "$INSTDIR\${FLEET_SIDECAR}.old-$R0"
    ${If} ${Errors}
      DetailPrint "Could not move $INSTDIR\${FLEET_SIDECAR} aside"
    ${Else}
      ; Succeeds right away when nothing was running it
      Delete "$INSTDIR\${FLEET_SIDECAR}.old-$R0"
    ${EndIf}
  ${EndIf}
  ClearErrors
  Pop $R0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro FLEET_EDIT_USER_PATH add
!macroend

; PATH removal sits in POST, not PRE: PRE runs before the template's
; "app is running" prompt, and cancelling that aborts the uninstall.
!macro NSIS_HOOK_POSTUNINSTALL
  ; An update reinstalls into the same directory right after, so keep PATH
  ${If} $UpdateMode <> 1
    !insertmacro FLEET_EDIT_USER_PATH remove
  ${EndIf}
  ; Files still held by running processes; /REBOOTOK only takes effect for
  ; per-machine installs, otherwise the next install's sweep gets them.
  Delete /REBOOTOK "$INSTDIR\${FLEET_SIDECAR}.old-*"
  ClearErrors
!macroend
