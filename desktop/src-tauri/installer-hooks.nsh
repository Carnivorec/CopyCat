!macro NSIS_HOOK_POSTINSTALL
  ${IfNot} ${Silent}
    DetailPrint "检查 Windows 防火墙…"
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --firewall allow' $0
  ${EndIf}
!macroend
