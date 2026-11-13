; Tauri NSIS 安装器钩子（在 `tauri.conf.json` 的 bundle.windows.nsis.installerHooks 里挂载）。
;
; 为什么需要它：Tauri 自带的 CHECK_APP_RUNNING 只按**主程序名**查进程，
; 杀掉 Threadline.exe 就以为清理干净了。但 tl-core.exe 是独立进程——
; 托盘退出后它可能还活着，开机自启时更是 SYSTEM 身份常驻。
; 于是安装包执行 `File` 覆盖 tl-core.exe 时撞上文件锁，
; 用户看到的是「无法打开要写入的文件 …\tl-core.exe」，升级直接失败。
;
; 这里在解压之前主动收尾。只用 nsExec（NSIS 自带），不依赖 nsProcess 插件
; ——插件在这个钩子的上下文里没有注册，调用会直接编译失败。
; taskkill 的退出码：0 = 已结束，128 = 本来就没在跑，其它 = 失败（多半是权限）。

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "正在关闭运行中的 ${MAINBINARYNAME} …"

  ; /T 连同子进程；先结束壳再结束核心，避免核心被壳重新拉起
  nsExec::ExecToLog 'taskkill /F /IM "${MAINBINARYNAME}.exe" /T'
  Pop $R0

  nsExec::ExecToLog 'taskkill /F /IM "tl-core.exe" /T'
  Pop $R0

  ; 等 Windows 释放映像文件句柄：太快解压仍会撞锁
  Sleep 900

  ; 128 是「没找到进程」，属于正常（本来就没开）。
  ; 其它非 0 说明进程在、但杀不掉（典型：开机自启的 SYSTEM 计划任务），
  ; 这时继续装下去必然在同一个文件上失败，先把原因说清楚。
  ${If} $R0 != 0
  ${AndIf} $R0 != 128
    DetailPrint "警告：tl-core.exe 未能结束（退出码 $R0，可能以 SYSTEM 身份运行）"
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION|MB_OK \
        "后台核心 tl-core.exe 仍在运行，安装程序没有权限结束它。$\r$\n$\r$\n通常是「开机自启」把它注册成了系统任务。请先在应用设置里关掉「开机自启」，或在任务管理器（勾选「显示所有用户的进程」）结束 tl-core.exe 后重试。"
    ${EndIf}
  ${EndIf}
!macroend

; 卸载同样会遇到占用：留着文件，卸载器就会提示「稍后重启才能删除」。
!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'taskkill /F /IM "${MAINBINARYNAME}.exe" /T'
  Pop $R0
  nsExec::ExecToLog 'taskkill /F /IM "tl-core.exe" /T'
  Pop $R0
  Sleep 700
!macroend
