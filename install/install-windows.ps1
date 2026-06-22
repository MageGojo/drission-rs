# =============================================================================
# drission · Windows 一键配置 Rust 开发环境(国内 rsproxy 镜像加速)
# 由 install-windows.bat 调起(双击 .bat 即可)。无需管理员权限(rustup 装到用户目录)。
#
# 关键选型:默认安装 **x86_64-pc-windows-gnu** 工具链 —— 它自带链接器与 MinGW 运行时,
# 编译纯 Rust 项目(含 drission 默认 cdp 后端)**无需安装 Visual Studio / Build Tools**,
# 真正“双击即用、不挑环境”。若日后要用 impersonate(BoringSSL)等需 C 编译的功能,
# 再按 README 装 VS Build Tools + nasm + cmake 即可。
# =============================================================================
$ErrorActionPreference = 'Stop'

$RSProxy = 'https://rsproxy.cn'
$env:RUSTUP_DIST_SERVER = $RSProxy
$env:RUSTUP_UPDATE_ROOT  = "$RSProxy/rustup"
$CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }

function Step($m){ Write-Host "`n==> $m" -ForegroundColor Cyan }
function OK($m){   Write-Host "[OK] $m"  -ForegroundColor Green }
function Warn($m){ Write-Host "[!]  $m"  -ForegroundColor Yellow }
function Err($m){  Write-Host "[X]  $m"  -ForegroundColor Red }

Write-Host "drission - Windows 一键 Rust 环境配置" -ForegroundColor White
Write-Host "镜像: $RSProxy   安装位置: $CargoHome"
Write-Host "工具链: x86_64-pc-windows-gnu(自带链接器,免装 Visual Studio);无需管理员权限。"

$haveRust = (Get-Command rustc -ErrorAction SilentlyContinue) -and (Get-Command cargo -ErrorAction SilentlyContinue)

if (-not $haveRust) {
    # -------------------------------------------------------------------------
    # [1/4] 下载 rustup-init.exe(rsproxy 国内镜像,官方源兜底)
    # -------------------------------------------------------------------------
    Step "[1/4] 下载 rustup-init.exe(国内镜像)"
    $arch   = if ([Environment]::Is64BitOperatingSystem) { 'x86_64' } else { 'i686' }
    $triple = "$arch-pc-windows-gnu"
    $init   = Join-Path $env:TEMP 'rustup-init.exe'
    if (Test-Path $init) { Remove-Item $init -Force -ErrorAction SilentlyContinue }
    $urls = @(
        "$RSProxy/rustup/dist/$triple/rustup-init.exe",
        "https://static.rust-lang.org/rustup/dist/$triple/rustup-init.exe"
    )
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue   # Win10+ 自带,带进度条
    $got  = $false
    foreach ($u in $urls) {
        try {
            Write-Host "  下载: $u"
            if ($curl) { & curl.exe -fL --progress-bar -o $init $u }
            else       { Invoke-WebRequest -Uri $u -OutFile $init }
            if ((Test-Path $init) -and ((Get-Item $init).Length -gt 100000)) { $got = $true; OK "已下载 rustup-init.exe"; break }
        } catch { Warn "该源下载失败:$($_.Exception.Message)" }
    }
    if (-not $got) { Err "rustup-init.exe 下载失败,请检查网络后重试。"; exit 1 }

    # -------------------------------------------------------------------------
    # [2/4] 安装 Rust 工具链(gnu,带进度,免装 VS;-y 非交互)
    # -------------------------------------------------------------------------
    Step "[2/4] 安装 Rust 工具链(带进度,免装 Visual Studio)"
    & $init -y --default-host $triple --default-toolchain stable --profile minimal --no-modify-path
    if ($LASTEXITCODE -ne 0) { Err "rustup 安装失败(退出码 $LASTEXITCODE)"; exit 1 }
    OK "rustup 安装完成"
    # 当前会话立即可用 + 持久化到用户 PATH(setx 用户级,无需管理员)
    $env:Path = "$CargoHome\bin;$env:Path"
    $userPath = [Environment]::GetEnvironmentVariable('Path','User')
    if ($userPath -notlike "*$CargoHome\bin*") {
        [Environment]::SetEnvironmentVariable('Path', "$CargoHome\bin;$userPath", 'User')
        OK "已把 $CargoHome\bin 加入用户 PATH"
    }
} else {
    Step "[1/4] 已检测到 Rust,跳过安装"
    OK (& rustc --version)
}

# -----------------------------------------------------------------------------
# [3/4] 配置 cargo 国内镜像 + 持久化 rustup 镜像变量(用户级)
# -----------------------------------------------------------------------------
Step "[3/4] 配置 cargo 国内镜像(拉依赖加速)"
New-Item -ItemType Directory -Force -Path $CargoHome | Out-Null
$cfg = Join-Path $CargoHome 'config.toml'
if ((Test-Path $cfg) -and (Select-String -Path $cfg -Pattern 'rsproxy' -Quiet)) {
    OK "已有 rsproxy 镜像配置,跳过"
} else {
    if (Test-Path $cfg) { Copy-Item $cfg "$cfg.bak.$(Get-Date -Format 'yyyyMMddHHmmss')"; Warn "已备份原 config.toml" }
    $conf = @'
[source.crates-io]
replace-with = "rsproxy-sparse"
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[registries.rsproxy]
index = "sparse+https://rsproxy.cn/index/"
[net]
git-fetch-with-cli = true
'@
    # 用 ASCII(内容全 ASCII)写,避免 UTF-8 BOM 让 cargo 解析 config.toml 出错。
    Set-Content -Path $cfg -Value $conf -Encoding ascii
    OK "已写入镜像配置: $cfg"
}
setx RUSTUP_DIST_SERVER $RSProxy            | Out-Null
setx RUSTUP_UPDATE_ROOT "$RSProxy/rustup"   | Out-Null

# -----------------------------------------------------------------------------
# [4/4] 验证(读取版本号 —— 能打印版本号即成功)
# -----------------------------------------------------------------------------
Step "[4/4] 验证安装(读取版本号)"
$fail = $false
$rustc = Get-Command rustc -ErrorAction SilentlyContinue
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($rustc) { OK ("rustc: " + (& rustc --version)) } else { Err "rustc 未就绪"; $fail = $true }
if ($cargo) { OK ("cargo: " + (& cargo --version)) } else { Err "cargo 未就绪"; $fail = $true }

Write-Host ""
if (-not $fail) {
    Write-Host "[完成] 环境配置成功!" -ForegroundColor Green
    Write-Host "请【新开】一个命令行/PowerShell 窗口(让 PATH 生效),然后:" -ForegroundColor White
    Write-Host "  cargo new demo; cd demo; cargo add drission; cargo run"
    Write-Host "预编译示例(免编译):见 dist\win 目录。"
    exit 0
} else {
    Warn "部分步骤未完成。请重开终端重试,或把以上红色信息发我。"
    exit 1
}
