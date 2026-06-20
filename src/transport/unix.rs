//! unix 平台的子进程 + fd3/fd4 异步管道实现。
//!
//! 关键点(易错,务必保持):
//! 1. 用 `os_pipe` 建两条匿名管道(默认 `O_CLOEXEC`,不会泄漏给子进程)。
//! 2. 用 `command-fds` 把"子进程那一端"映射到固定的 fd3 / fd4(它内部做了
//!    fd 重排与去 `CLOEXEC`,正确处理与 3/4 冲突的边界)。
//! 3. **spawn 后立刻 `drop(command)`**:`command-fds` 把子进程端 fd 的所有权交给了
//!    `Command` 内部的 `pre_exec` 闭包;若不丢弃,父进程会一直持有 resp 管道的写端副本,
//!    导致子进程退出时父进程读端**永不 EOF**(断连检测失效)。
//! 4. 父进程保留的两端用 tokio `pipe::Sender/Receiver` 包成异步读写;
//!    `from_owned_fd` 会校验是管道并自动设为非阻塞。

use std::path::Path;
use std::process::Stdio;

use command_fds::{CommandFdExt, FdMapping};
use tokio::net::unix::pipe;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};

use crate::{Error, Result};

/// 已启动的浏览器子进程及其异步通信端。
pub struct Spawned {
    /// 子进程句柄(`kill_on_drop` 已开启)。
    pub child: Child,
    /// 写命令的一端 → 子进程 fd3。
    pub writer: pipe::Sender,
    /// 读响应的一端 ← 子进程 fd4。
    pub reader: pipe::Receiver,
    /// 子进程 stdout(异步)。Camoufox/Firefox 的就绪标记可能打在 stdout 或 stderr。
    pub stdout: ChildStdout,
    /// 子进程 stderr(异步),用于就绪检测与后续日志。
    pub stderr: ChildStderr,
}

/// 启动 `program` 并接好 Juggler 的 fd3/fd4 管道。
///
/// 注意:必须在 **tokio 运行时**内调用(内部 `pipe::Sender/Receiver::from_owned_fd`
/// 需要向 I/O 反应堆注册)。签名为 `async` 仅为与 windows 实现统一(本体无 await)。
#[allow(clippy::unused_async)]
pub async fn spawn(program: &Path, args: &[String], envs: &[(String, String)]) -> Result<Spawned> {
    // cmd 管道:父写 cmd_writer → 子读 cmd_reader(将成为 fd3)。
    let (cmd_reader, cmd_writer) =
        os_pipe::pipe().map_err(|e| Error::Transport(format!("创建 cmd 管道失败: {e}")))?;
    // resp 管道:子写 resp_writer(将成为 fd4)→ 父读 resp_reader。
    let (resp_reader, resp_writer) =
        os_pipe::pipe().map_err(|e| Error::Transport(format!("创建 resp 管道失败: {e}")))?;

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in envs {
        command.env(k, v);
    }

    command
        .fd_mappings(vec![
            FdMapping {
                parent_fd: cmd_reader.into(),
                child_fd: 3,
            },
            FdMapping {
                parent_fd: resp_writer.into(),
                child_fd: 4,
            },
        ])
        .map_err(|e| Error::Transport(format!("fd 映射冲突: {e}")))?;

    let mut child = command
        .spawn()
        .map_err(|e| Error::Transport(format!("启动子进程失败: {e}")))?;

    // 见模块文档第 3 点:必须丢弃 command 以关闭父进程持有的子进程端 fd 副本。
    drop(command);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Transport("无法捕获子进程 stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Transport("无法捕获子进程 stderr".into()))?;

    let writer = pipe::Sender::from_owned_fd(cmd_writer.into())
        .map_err(|e| Error::Transport(format!("包裹命令写端失败: {e}")))?;
    let reader = pipe::Receiver::from_owned_fd(resp_reader.into())
        .map_err(|e| Error::Transport(format!("包裹响应读端失败: {e}")))?;

    Ok(Spawned {
        child,
        writer,
        reader,
        stdout,
        stderr,
    })
}
