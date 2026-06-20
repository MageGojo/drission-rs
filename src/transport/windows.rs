//! windows 平台的子进程 + Juggler fd3/fd4 管道实现。
//!
//! 与 unix 的差异(关键,务必理解):
//! - **管道**:用 *命名管道*。父端是 tokio 的 [`NamedPipeServer`](tokio::net::windows::named_pipe::NamedPipeServer)
//!   (overlapped 异步);子端用同步、**可继承**的句柄(`CreateFileW`),供子进程阻塞读写。
//! - **fd 注入**:Camoufox/Firefox 的 Windows 入口(`wmain` 与 `LauncherProcessWin`)用
//!   `_get_osfhandle(3)` / `_get_osfhandle(4)` 取 Juggler 管道句柄,再经 `PW_PIPE_READ`/
//!   `PW_PIPE_WRITE` 环境变量传给真正的浏览器进程。所以父进程**必须**把句柄放进子进程的
//!   **CRT fd 表**——即 MSVCRT 的 `STARTUPINFO.lpReserved2` 句柄继承块(与 libuv/Node 同法)。
//!   `std::process::Command` 不暴露 `lpReserved2`,故这里直接调 `CreateProcessW`。
//!
//! 句柄到 fd 的映射(`lpReserved2` 块,按 fd 顺序):
//! - fd0 = stdin  → `NUL` 设备
//! - fd1 = stdout → out 管道(子写父读)
//! - fd2 = stderr → err 管道(子写父读;就绪标志 "Juggler listening" 在此出现)
//! - fd3 = juggler 读 → cmd 管道(父写子读)
//! - fd4 = juggler 写 → resp 管道(子写父读)
//!
//! 进程生命周期由 [`WinChild`] 管理(`wait`/`kill`/`start_kill` 与 tokio `Child` 同名,
//! 故 Camoufox 后端(`browser` / `launcher`)的消费代码两平台一致)。

use std::collections::BTreeMap;
use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::CreateFileW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};

use crate::{Error, Result};

// ── Win32 数值常量(自定义以免依赖 windows-sys 各特性的具体模块路径)─────────────
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const INFINITE: u32 = 0xFFFF_FFFF;

// MSVCRT lowio 的 fd 标志(`lpReserved2` 块里每个 fd 一字节)。
const FOPEN: u8 = 0x01;
const FPIPE: u8 = 0x08;
const FDEV: u8 = 0x40;

/// 已启动的浏览器子进程及其异步通信端(字段含义同 unix,只是父端类型为命名管道)。
pub struct Spawned {
    pub child: WinChild,
    /// 写命令的一端 → 子进程 fd3。
    pub writer: NamedPipeServer,
    /// 读响应的一端 ← 子进程 fd4。
    pub reader: NamedPipeServer,
    /// 子进程 stdout(异步)。
    pub stdout: NamedPipeServer,
    /// 子进程 stderr(异步),用于就绪检测与后续日志。
    pub stderr: NamedPipeServer,
}

/// windows 子进程句柄。析构时兜底终止并关闭句柄(等价 unix 的 `kill_on_drop(true)`)。
pub struct WinChild {
    process: HANDLE,
    thread: HANDLE,
    #[allow(dead_code)]
    pid: u32,
    exit_code: Option<u32>,
}

// `HANDLE` 是裸指针,默认非 Send/Sync;此处句柄仅经 `&mut self` 或 `Mutex` 串行访问,故安全。
unsafe impl Send for WinChild {}
unsafe impl Sync for WinChild {}

impl WinChild {
    /// 等待进程退出并返回退出码(可重复调用:首次后缓存结果)。
    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if let Some(code) = self.exit_code {
            return Ok(ExitStatus::from_raw(code));
        }
        let h = SendHandle(self.process);
        let code = tokio::task::spawn_blocking(move || {
            // 整体捕获 `SendHandle`(它是 Send);2024 edition 的字段级捕获会只捕获
            // `h.0` 裸指针(非 Send),故此处先把整个 `h` 搬进闭包再用。
            let h = h;
            unsafe {
                WaitForSingleObject(h.0, INFINITE);
                let mut code: u32 = 0;
                GetExitCodeProcess(h.0, &mut code);
                code
            }
        })
        .await
        .map_err(std::io::Error::other)?;
        self.exit_code = Some(code);
        Ok(ExitStatus::from_raw(code))
    }

    /// 发送终止信号(不等待回收)。进程可能已退出,失败不视为致命。
    pub fn start_kill(&mut self) -> std::io::Result<()> {
        if self.exit_code.is_none() {
            unsafe { TerminateProcess(self.process, 1) };
        }
        Ok(())
    }

    /// 终止并等待回收。
    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(|_| ())
    }
}

impl Drop for WinChild {
    fn drop(&mut self) {
        unsafe {
            if self.exit_code.is_none() {
                TerminateProcess(self.process, 1);
            }
            if !self.process.is_null() {
                CloseHandle(self.process);
            }
            if !self.thread.is_null() {
                CloseHandle(self.thread);
            }
        }
    }
}

/// 把裸 `HANDLE` 跨线程搬进 `spawn_blocking`(仅作整数搬运,不转移所有权)。
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

/// 启动 `program` 并接好 Juggler 的 fd3/fd4(命名管道 + CRT 句柄继承)。
///
/// 必须在 **tokio 运行时**内调用(命名管道父端注册到运行时的 IOCP)。
pub async fn spawn(program: &Path, args: &[String], envs: &[(String, String)]) -> Result<Spawned> {
    // 1) 建 4 条命名管道:父端 server(异步),子端 client(同步、可继承)。
    //    数据方向决定子端 client 的访问权限。
    let cmd = create_pipe("cmd", GENERIC_READ)?; // 父写 → 子读(fd3)
    let resp = create_pipe("resp", GENERIC_WRITE)?; // 子写 → 父读(fd4)
    let out = create_pipe("out", GENERIC_WRITE)?; // 子写 stdout → 父读(fd1)
    let err = create_pipe("err", GENERIC_WRITE)?; // 子写 stderr → 父读(fd2)

    // 2) stdin 接 NUL 设备(可继承只读句柄)。
    let nul = match open_inheritable("NUL", GENERIC_READ) {
        Ok(h) => h,
        Err(e) => {
            close_all(&[cmd.client, resp.client, out.client, err.client]);
            return Err(e);
        }
    };

    // 3) 等父端 server 接纳子端 client(client 已 CreateFile 连上,connect 立即返回)。
    let connect_res = async {
        cmd.server.connect().await?;
        resp.server.connect().await?;
        out.server.connect().await?;
        err.server.connect().await
    }
    .await;
    if let Err(e) = connect_res {
        close_all(&[nul, cmd.client, resp.client, out.client, err.client]);
        return Err(Error::Transport(format!("命名管道连接失败: {e}")));
    }

    // 4) 组装 CRT `lpReserved2` 句柄继承块(fd0..fd4)。
    let mut block = build_crt_block(&[
        (FOPEN | FDEV, nul),          // fd0 stdin
        (FOPEN | FPIPE, out.client),  // fd1 stdout
        (FOPEN | FPIPE, err.client),  // fd2 stderr
        (FOPEN | FPIPE, cmd.client),  // fd3 juggler 读
        (FOPEN | FPIPE, resp.client), // fd4 juggler 写
    ]);

    // 5) 组装命令行 / 应用名 / 环境块(UTF-16)。
    let app = to_wide(&program.to_string_lossy());
    let mut cmdline = build_command_line(program, args);
    let env_block = build_env_block(envs);

    // 6) STARTUPINFOW:标准句柄 + 句柄继承块。
    let mut si: STARTUPINFOW = unsafe { core::mem::zeroed() };
    si.cb = core::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = nul;
    si.hStdOutput = out.client;
    si.hStdError = err.client;
    si.cbReserved2 = block.len() as u16;
    si.lpReserved2 = block.as_mut_ptr();

    let mut pi: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };

    // 7) CreateProcessW:bInheritHandles=TRUE + Unicode 环境。
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            cmdline.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1, // bInheritHandles = TRUE
            CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *const c_void,
            core::ptr::null(), // 继承当前工作目录
            &si,
            &mut pi,
        )
    };

    // 8) 无论成败,父进程都关闭子端句柄副本(成功后子进程已持有继承副本;
    //    尤其子写端必须关掉父副本,否则父读端永不 EOF)。
    close_all(&[nul, cmd.client, resp.client, out.client, err.client]);

    if ok == 0 {
        let gle = unsafe { GetLastError() };
        return Err(Error::Transport(format!(
            "CreateProcessW 启动 Camoufox 失败(GetLastError={gle})"
        )));
    }

    Ok(Spawned {
        child: WinChild {
            process: pi.hProcess,
            thread: pi.hThread,
            pid: pi.dwProcessId,
            exit_code: None,
        },
        writer: cmd.server,
        reader: resp.server,
        stdout: out.server,
        stderr: err.server,
    })
}

/// 父端 server + 子端可继承 client 的一对命名管道。
struct PipePair {
    server: NamedPipeServer,
    client: HANDLE,
}

/// 建一条命名管道:tokio 父端 server(overlapped) + 同步可继承子端 client。
fn create_pipe(tag: &str, client_access: u32) -> Result<PipePair> {
    let name = unique_pipe_name(tag);
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&name)
        .map_err(|e| Error::Transport(format!("创建命名管道 {tag} 失败: {e}")))?;
    let client = open_inheritable(&name, client_access).map_err(|e| {
        // server 会随 PipePair 未构造而被 drop 关闭。
        Error::Transport(format!("打开命名管道 {tag} 子端失败: {e}"))
    })?;
    Ok(PipePair { server, client })
}

/// 用 `CreateFileW` 打开一个**可继承**(`bInheritHandle=TRUE`)、同步的句柄。
fn open_inheritable(name: &str, access: u32) -> Result<HANDLE> {
    let wide = to_wide(name);
    let sa = SECURITY_ATTRIBUTES {
        nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: core::ptr::null_mut(),
        bInheritHandle: 1, // TRUE
    };
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &sa,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL, // 同步句柄(无 OVERLAPPED),供子进程阻塞读写
            core::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        let gle = unsafe { GetLastError() };
        return Err(Error::Transport(format!(
            "CreateFileW({name}) 失败(GetLastError={gle})"
        )));
    }
    Ok(h)
}

/// 生成进程内唯一的命名管道名。
fn unique_pipe_name(tag: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        r"\\.\pipe\drission-juggler-{}-{}-{}-{}",
        std::process::id(),
        nanos,
        n,
        tag
    )
}

/// 构造 MSVCRT 的 `lpReserved2` 句柄继承块(紧凑布局,与 CRT 读取一致):
/// `u32 count` + `count` 字节 flags + `count` 个指针宽 handle(均不对齐)。
fn build_crt_block(entries: &[(u8, HANDLE)]) -> Vec<u8> {
    let count = entries.len();
    let mut buf = Vec::with_capacity(4 + count + count * core::mem::size_of::<HANDLE>());
    buf.extend_from_slice(&(count as u32).to_ne_bytes());
    for (flags, _) in entries {
        buf.push(*flags);
    }
    for (_, h) in entries {
        buf.extend_from_slice(&(*h as usize).to_ne_bytes());
    }
    buf
}

/// UTF-16 + 末尾 NUL。
fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 组装 `CreateProcessW` 的命令行(argv0=程序路径,随后各参数,按 Windows 规则转义)。
fn build_command_line(program: &Path, args: &[String]) -> Vec<u16> {
    let mut s = String::new();
    append_arg(&mut s, &program.to_string_lossy());
    for a in args {
        s.push(' ');
        append_arg(&mut s, a);
    }
    OsStr::new(&s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 单个参数按 Windows `CommandLineToArgvW` 规则追加(必要时加引号、转义反斜杠/引号)。
fn append_arg(out: &mut String, arg: &str) {
    let needs_quote = arg.is_empty() || arg.contains([' ', '\t', '"']);
    if !needs_quote {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..(backslashes * 2 + 1) {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                out.push(c);
                backslashes = 0;
            }
        }
    }
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
}

/// 组装 UTF-16 环境块:父进程环境 + `extra`(覆盖同名,大小写不敏感),按名排序、双 NUL 结尾。
fn build_env_block(extra: &[(String, String)]) -> Vec<u16> {
    let mut map: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (k, v) in std::env::vars() {
        map.insert(k.to_uppercase(), (k, v));
    }
    for (k, v) in extra {
        map.insert(k.to_uppercase(), (k.clone(), v.clone()));
    }
    let mut block: Vec<u16> = Vec::new();
    for (_, (k, v)) in map {
        let entry = format!("{k}={v}");
        block.extend(OsStr::new(&entry).encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0); // 结尾双 NUL
    block
}

/// 批量关闭句柄(忽略错误)。
fn close_all(handles: &[HANDLE]) {
    for &h in handles {
        if !h.is_null() && h != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(h) };
        }
    }
}
