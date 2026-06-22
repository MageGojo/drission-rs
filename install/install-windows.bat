@echo off
REM ============================================================================
REM drission - Windows 一键配置 Rust 开发环境(国内 rsproxy 镜像加速)
REM
REM 给谁用:不熟悉 Rust 的 Python / TS 等开发者,想快速体验 drission。
REM 怎么用:**双击本文件**即可(它会调起 PowerShell 完成安装,带进度)。
REM 做了啥:用 rsproxy 国内镜像安装 rustup + Rust(gnu 工具链,自带链接器、免装 Visual Studio),
REM         并配置 cargo 走国内镜像,最后验证 rustc / cargo 版本号。
REM 权限:  装在你的用户目录,**无需管理员权限**(故不会弹 UAC)。不依赖任何包管理器。
REM ============================================================================
setlocal
cd /d "%~dp0"
echo.
echo   drission - Windows 一键 Rust 环境配置(国内镜像加速)
echo   即将启动 PowerShell 安装程序...
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-windows.ps1"
set "rc=%ERRORLEVEL%"
echo.
if "%rc%"=="0" (
  echo   完成。可关闭本窗口。
) else (
  echo   安装未完全成功(退出码 %rc%^)。可重试,或把上面的信息发给我们。
)
echo.
echo   (按任意键关闭本窗口)
pause >nul
endlocal
