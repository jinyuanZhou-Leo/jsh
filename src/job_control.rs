use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd},
    os::unix::process::CommandExt,
    process::Command,
};

use nix::{
    errno::Errno,
    sys::{
        signal::{
            SaFlags, SigAction, SigHandler, SigSet, SigmaskHow, Signal, killpg, sigaction,
            sigprocmask,
        },
        termios::{SetArg, Termios, tcgetattr, tcsetattr},
        wait::{WaitPidFlag, WaitStatus, waitpid},
    },
    unistd::{Pid, getpgrp, getpid, isatty, setpgid, tcgetpgrp, tcsetpgrp},
};
use thiserror::Error;

const SHELL_SIGNALS: [Signal; 5] = [
    Signal::SIGINT,
    Signal::SIGQUIT,
    Signal::SIGTSTP,
    Signal::SIGTTIN,
    Signal::SIGTTOU,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct JobId(u32);

impl std::fmt::Display for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    Running,
    Stopped(Signal),
    Completed(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobStage {
    Process(Pid),
    Completed(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessRecord {
    pid: Pid,
    state: ProcessState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeStage {
    Process(ProcessRecord),
    Completed(i32),
}

impl From<JobStage> for RuntimeStage {
    fn from(stage: JobStage) -> Self {
        match stage {
            JobStage::Process(pid) => Self::Process(ProcessRecord {
                pid,
                state: ProcessState::Running,
            }),
            JobStage::Completed(status) => Self::Completed(status),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Running,
    Stopped,
    Done,
}

#[derive(Debug)]
struct Job {
    id: JobId,
    pgid: Pid,
    command_text: String,
    stages: Vec<RuntimeStage>,
    state: JobState,
    terminal_modes: Option<Termios>,
    notified: bool,
}

impl Job {
    /// 创建一条已经发布到运行期作业表的作业记录。
    ///
    /// # Arguments
    ///
    /// * `id` - 供 Shell 用户界面引用的作业编号。
    /// * `pgid` - 作业中所有进程所属的进程组 ID。
    /// * `command_text` - `jobs` 和状态通知使用的稳定展示文本。
    /// * `stages` - 已启动进程和无需启动进程的初始阶段状态。
    ///
    /// # Returns
    ///
    /// 根据初始阶段聚合出状态的新作业记录。
    fn new(
        id: JobId,
        pgid: Pid,
        command_text: String,
        stages: Vec<JobStage>,
    ) -> Self {
        let mut job = Self {
            id,
            pgid,
            command_text,
            stages: stages.into_iter().map(RuntimeStage::from).collect(),
            state: JobState::Running,
            terminal_modes: None,
            notified: false,
        };
        job.recompute_state();
        job
    }

    /// 根据所有阶段状态重新计算作业级 `Running`、`Stopped` 或 `Done` 状态。
    fn recompute_state(&mut self) {
        let mut running = false;
        let mut stopped = false;

        for stage in &self.stages {
            match stage {
                RuntimeStage::Process(ProcessRecord {
                    state: ProcessState::Running,
                    ..
                }) => running = true,
                RuntimeStage::Process(ProcessRecord {
                    state: ProcessState::Stopped(_),
                    ..
                }) => stopped = true,
                RuntimeStage::Process(ProcessRecord {
                    state: ProcessState::Completed(_),
                    ..
                })
                | RuntimeStage::Completed(_) => {}
            }
        }

        self.state = if running {
            JobState::Running
        } else if stopped {
            JobState::Stopped
        } else {
            JobState::Done
        };
    }

    /// 将一个内核等待事件应用到对应的进程记录。
    ///
    /// # Arguments
    ///
    /// * `status` - `waitpid` 返回的进程状态变化。
    ///
    /// # Returns
    ///
    /// 事件属于本作业并更新了进程状态时返回 `true`；否则返回 `false`。
    fn apply_wait_status(&mut self, status: WaitStatus) -> bool {
        let Some(pid) = wait_status_pid(&status) else {
            return false;
        };
        let Some(process) = self.stages.iter_mut().find_map(|stage| match stage {
            RuntimeStage::Process(process) if process.pid == pid => Some(process),
            RuntimeStage::Process(_) | RuntimeStage::Completed(_) => None,
        }) else {
            return false;
        };

        process.state = match status {
            WaitStatus::Exited(_, code) => ProcessState::Completed(code),
            WaitStatus::Signaled(_, signal, _) => ProcessState::Completed(128 + signal as i32),
            WaitStatus::Stopped(_, signal) => ProcessState::Stopped(signal),
            WaitStatus::Continued(_) => ProcessState::Running,
            WaitStatus::StillAlive => return false,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            WaitStatus::PtraceEvent(_, _, _) | WaitStatus::PtraceSyscall(_) => return false,
        };
        self.notified = false;
        self.recompute_state();
        true
    }

    /// 返回作业最后一个阶段对应的 Shell 状态码。
    ///
    /// # Returns
    ///
    /// 正常退出使用原始退出码；信号终止或停止使用 `128 + signal`；仍在运行时返回 0。
    fn status_code(&self) -> i32 {
        match self.stages.last() {
            Some(RuntimeStage::Completed(status)) => *status,
            Some(RuntimeStage::Process(ProcessRecord {
                state: ProcessState::Completed(status),
                ..
            })) => *status,
            Some(RuntimeStage::Process(ProcessRecord {
                state: ProcessState::Stopped(signal),
                ..
            })) => 128 + *signal as i32,
            Some(RuntimeStage::Process(ProcessRecord {
                state: ProcessState::Running,
                ..
            }))
            | None => 0,
        }
    }

    /// 在发送 `SIGCONT` 后把所有已停止进程标记为运行中。
    fn mark_running(&mut self) {
        for stage in &mut self.stages {
            if let RuntimeStage::Process(process) = stage
                && matches!(process.state, ProcessState::Stopped(_))
            {
                process.state = ProcessState::Running;
            }
        }
        self.state = JobState::Running;
        self.notified = false;
    }

    /// 构造 `jobs` 和异步状态通知使用的一行文本。
    ///
    /// # Returns
    ///
    /// 包含 JobId、聚合状态和原始命令文本的自包含字符串。
    fn display_line(&self) -> String {
        let state = match self.state {
            JobState::Running => "Running",
            JobState::Stopped => "Stopped",
            JobState::Done => "Done",
        };
        format!("[{}] {state:<7} {}", self.id, self.command_text)
    }
}

#[derive(Debug)]
struct InteractiveSession {
    terminal: File,
    shell_pgid: Pid,
    shell_terminal_modes: Termios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobControlMode {
    NonInteractive,
    Interactive,
    Subshell { pgid: Pid },
}

#[derive(Debug)]
pub(crate) struct JobControl {
    mode: JobControlMode,
    session: Option<InteractiveSession>,
    jobs: BTreeMap<JobId, Job>,
    next_job_id: u32,
}

#[derive(Debug, Error)]
pub(crate) enum JobControlError {
    #[error("failed to inspect standard input: {0}")]
    InspectStdin(Errno),
    #[error("failed to open controlling terminal: {0}")]
    OpenTerminal(#[source] io::Error),
    #[error("failed to initialize shell process group: {0}")]
    InitializeProcessGroup(Errno),
    #[error("failed to configure signal {signal:?}: {source}")]
    ConfigureSignal { signal: Signal, source: Errno },
    #[error("failed to access controlling terminal: {0}")]
    Terminal(Errno),
    #[error("failed to wait for a child process: {0}")]
    Wait(Errno),
    #[error("no current job")]
    NoCurrentJob,
    #[error("invalid job specification `{0}`")]
    InvalidJobSpec(String),
    #[error("job %{0} does not exist")]
    UnknownJob(JobId),
    #[error("job %{0} is not stopped")]
    JobNotStopped(JobId),
    #[error("failed to signal job %{job_id}: {source}")]
    SignalJob { job_id: JobId, source: Errno },
}

impl JobControl {
    /// 创建尚未绑定控制终端的作业控制器。
    ///
    /// # Returns
    ///
    /// 作业表为空、下一个 JobId 为 1 的非交互控制器。
    pub(crate) fn new() -> Self {
        Self {
            mode: JobControlMode::NonInteractive,
            session: None,
            jobs: BTreeMap::new(),
            next_job_id: 1,
        }
    }

    /// 在终端会话中初始化 Shell 进程组、信号策略和终端所有权。
    ///
    /// 标准输入不是终端时保持非交互模式并直接成功返回。
    ///
    /// # Errors
    ///
    /// 检查标准输入、打开 `/dev/tty`、配置进程组或信号、读取或转移终端状态失败时返回错误。
    pub(crate) fn initialize_interactive(&mut self) -> Result<(), JobControlError> {
        if !isatty(io::stdin().as_fd()).map_err(JobControlError::InspectStdin)? {
            return Ok(());
        }

        let terminal = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(JobControlError::OpenTerminal)?;

        while tcgetpgrp(&terminal).map_err(JobControlError::Terminal)? != getpgrp() {
            killpg(getpgrp(), Signal::SIGTTIN).map_err(JobControlError::InitializeProcessGroup)?;
        }

        install_shell_signal_dispositions()?;

        let shell_pid = getpid();
        if getpgrp() != shell_pid {
            setpgid(shell_pid, shell_pid).map_err(JobControlError::InitializeProcessGroup)?;
        }
        tcsetpgrp(&terminal, shell_pid).map_err(JobControlError::Terminal)?;
        let shell_terminal_modes = tcgetattr(&terminal).map_err(JobControlError::Terminal)?;

        self.mode = JobControlMode::Interactive;
        self.session = Some(InteractiveSession {
            terminal,
            shell_pgid: shell_pid,
            shell_terminal_modes,
        });
        Ok(())
    }

    /// 将控制器切换为后台复合作业使用的隔离子 Shell 模式。
    ///
    /// # Arguments
    ///
    /// * `pgid` - 子 Shell 及其后续子进程必须加入的作业进程组。
    pub(crate) fn become_subshell(&mut self, pgid: Pid) {
        self.mode = JobControlMode::Subshell { pgid };
        self.session = None;
        self.jobs.clear();
    }

    /// 在 fork 后的后台监督进程中建立独立进程组并恢复子进程信号语义。
    ///
    /// # Arguments
    ///
    /// * `pgid` - 当前监督进程应加入的作业进程组，通常等于其 PID。
    ///
    /// # Errors
    ///
    /// 建立进程组、恢复默认信号 disposition 或清空信号 mask 失败时返回错误。
    pub(crate) fn prepare_subshell_process(pgid: Pid) -> Result<(), JobControlError> {
        setpgid(Pid::from_raw(0), pgid).map_err(JobControlError::InitializeProcessGroup)?;
        reset_child_signal_state().map_err(|source| JobControlError::ConfigureSignal {
            signal: Signal::SIGCHLD,
            source,
        })
    }

    /// 判断控制器是否持有交互式 Shell 会话。
    ///
    /// # Returns
    ///
    /// 当前处于交互模式时返回 `true`。
    pub(crate) fn is_interactive(&self) -> bool {
        self.mode == JobControlMode::Interactive
    }

    /// 判断控制器是否处于没有终端 Job Control 的顶层非交互模式。
    ///
    /// # Returns
    ///
    /// 当前为顶层非交互模式时返回 `true`；隔离子 Shell 模式返回 `false`。
    pub(crate) fn is_non_interactive(&self) -> bool {
        self.mode == JobControlMode::NonInteractive
    }

    /// 计算下一个子进程应加入的进程组。
    ///
    /// # Arguments
    ///
    /// * `current_job_pgid` - 已启动 pipeline 阶段确定的 PGID；首阶段传入 `None`。
    ///
    /// # Returns
    ///
    /// 交互模式首阶段返回 PID 0 作为“以自身 PID 建组”的标记，后续阶段返回现有 PGID；
    /// 子 Shell 返回其固定 PGID；非交互顶层命令返回 `None`。
    pub(crate) fn child_group_target(&self, current_job_pgid: Option<Pid>) -> Option<Pid> {
        match self.mode {
            JobControlMode::Interactive => {
                Some(current_job_pgid.unwrap_or_else(|| Pid::from_raw(0)))
            }
            JobControlMode::Subshell { pgid } => Some(pgid),
            JobControlMode::NonInteractive => None,
        }
    }

    /// 为尚未启动的 `std::process::Command` 安装 fork 后、exec 前的 Job Control 设置。
    ///
    /// # Arguments
    ///
    /// * `command` - 需要附加 `pre_exec` 设置的子进程命令。
    /// * `target_pgid` - 子进程目标 PGID；`None` 表示不配置进程组。
    /// * `foreground` - 是否需要在 exec 前把控制终端交给目标进程组。
    ///
    /// 此函数本身不启动进程；系统调用错误将在随后调用 `Command::spawn` 时返回。
    pub(crate) fn configure_child_command(
        &self,
        command: &mut Command,
        target_pgid: Option<Pid>,
        foreground: bool,
    ) {
        let Some(target_pgid) = target_pgid else {
            return;
        };
        let pgid_raw = target_pgid.as_raw();
        let terminal_raw = self
            .session
            .as_ref()
            .filter(|_| foreground)
            .map(|session| session.terminal.as_raw_fd());

        // SAFETY: the closure only invokes async-signal-safe process, signal, and terminal
        // syscalls. All captured values are plain integers and no allocation is performed.
        unsafe {
            command.pre_exec(move || {
                let child_pid = getpid();
                let child_pgid = if pgid_raw == 0 {
                    child_pid
                } else {
                    Pid::from_raw(pgid_raw)
                };
                setpgid(Pid::from_raw(0), child_pgid).map_err(errno_to_io)?;

                if let Some(terminal_raw) = terminal_raw {
                    // SAFETY: the descriptor belongs to the inherited /dev/tty File and is
                    // valid until exec closes it.
                    let terminal = BorrowedFd::borrow_raw(terminal_raw);
                    tcsetpgrp(terminal, child_pgid).map_err(errno_to_io)?;
                }
                // tcsetpgrp must run while the child still inherits the Shell's ignored
                // SIGTTOU disposition; restoring SIGTTOU first would stop the child in pre_exec.
                reset_child_signal_state().map_err(errno_to_io)?;
                Ok(())
            });
        }
    }

    /// 在父进程侧确认子进程已经加入目标进程组，消除父子调度竞态。
    ///
    /// # Arguments
    ///
    /// * `child` - 已启动子进程的 PID。
    /// * `target_pgid` - 启动前选择的目标 PGID；PID 0 表示使用 `child` 自身建组。
    ///
    /// # Returns
    ///
    /// 后续登记和等待应使用的实际 PGID。
    ///
    /// # Errors
    ///
    /// `setpgid` 返回无法由正常 exec/退出竞态解释的错误时返回错误。
    pub(crate) fn confirm_child_process_group(
        &self,
        child: Pid,
        target_pgid: Option<Pid>,
    ) -> Result<Pid, JobControlError> {
        let Some(target_pgid) = target_pgid else {
            return Ok(child);
        };
        let actual_pgid = if target_pgid.as_raw() == 0 {
            child
        } else {
            target_pgid
        };

        match setpgid(child, actual_pgid) {
            Ok(()) | Err(Errno::EACCES) | Err(Errno::ESRCH) => Ok(actual_pgid),
            Err(error) => Err(JobControlError::InitializeProcessGroup(error)),
        }
    }

    /// 把已经启动的进程组发布为可由 Shell 管理的 Job。
    ///
    /// # Arguments
    ///
    /// * `pgid` - 作业的唯一进程组 ID。
    /// * `command_text` - 用户可读的命令展示文本。
    /// * `stages` - 按 pipeline 顺序排列的运行期阶段。
    ///
    /// # Returns
    ///
    /// 分配给新作业的 Shell JobId。
    pub(crate) fn register_job(
        &mut self,
        pgid: Pid,
        command_text: String,
        stages: Vec<JobStage>,
    ) -> JobId {
        let id = JobId(self.next_job_id);
        self.next_job_id = self.next_job_id.saturating_add(1);
        self.jobs
            .insert(id, Job::new(id, pgid, command_text, stages));
        id
    }

    /// 把指定作业置于前台，并等待它完全停止或完成。
    ///
    /// # Arguments
    ///
    /// * `job_id` - 需要转到前台的作业编号。
    /// * `continue_stopped` - 是否先向已停止作业发送 `SIGCONT`。
    ///
    /// # Returns
    ///
    /// 作业最后一个阶段的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 作业不存在，终端转移、发送信号、等待或恢复终端状态失败时返回错误。
    pub(crate) fn wait_for_foreground_job(
        &mut self,
        job_id: JobId,
        continue_stopped: bool,
    ) -> Result<i32, JobControlError> {
        if let Err(error) = self.give_terminal_to_job(job_id, continue_stopped) {
            let _ = self.reclaim_terminal(job_id);
            return Err(error);
        }
        let wait_result = self.wait_until_stopped_or_done(job_id);
        let reclaim_result = self.reclaim_terminal(job_id);

        reclaim_result?;
        wait_result
    }

    /// 在没有交互式 Job Control 时逐一回收已经全部启动的阶段。
    ///
    /// # Arguments
    ///
    /// * `stages` - 按 pipeline 顺序排列的 PID 或预先完成状态。
    ///
    /// # Returns
    ///
    /// 最后一个阶段的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// 任一子进程无法被 `waitpid` 回收时，在继续尝试回收其他阶段后返回首个错误。
    pub(crate) fn wait_for_stages(stages: &mut [JobStage]) -> Result<i32, JobControlError> {
        let mut last_status = 0;
        let mut first_error = None;

        for stage in stages {
            match stage {
                JobStage::Completed(status) => last_status = *status,
                JobStage::Process(pid) => loop {
                    match waitpid(*pid, None) {
                        Ok(WaitStatus::Exited(_, status)) => {
                            last_status = status;
                            break;
                        }
                        Ok(WaitStatus::Signaled(_, signal, _)) => {
                            last_status = 128 + signal as i32;
                            break;
                        }
                        Ok(_) => {}
                        Err(Errno::EINTR) => continue,
                        Err(error) => {
                            first_error.get_or_insert(error);
                            break;
                        }
                    }
                },
            }
        }

        first_error.map_or(Ok(last_status), |error| Err(JobControlError::Wait(error)))
    }

    /// 非阻塞回收本控制器已登记进程组中的状态变化。
    ///
    /// 只按 JobTable 中的 PGID 调用 `waitpid`，不会回收其他 Shell 实例拥有的子进程。
    ///
    /// # Errors
    ///
    /// `waitpid` 返回除中断和“无子进程”之外的错误时返回错误。
    pub(crate) fn reap_nonblocking(&mut self) -> Result<(), JobControlError> {
        let flags = WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED;
        let process_groups: Vec<_> = self.jobs.values().map(|job| job.pgid).collect();

        // Never use waitpid(-1) here: multiple Shell instances can coexist in tests or an
        // embedding process, and a global reap could steal a child owned by another instance.
        for pgid in process_groups {
            loop {
                match waitpid(Pid::from_raw(-pgid.as_raw()), Some(flags)) {
                    Ok(WaitStatus::StillAlive) | Err(Errno::ECHILD) => break,
                    Err(Errno::EINTR) => continue,
                    Err(error) => return Err(JobControlError::Wait(error)),
                    Ok(status) => self.apply_wait_status(status),
                }
            }
        }
        Ok(())
    }

    /// 收集尚未通知的停止/完成状态，并移除已通知的完成作业。
    ///
    /// # Returns
    ///
    /// 按 JobId 排列、可直接输出到 REPL 的状态行。
    ///
    /// # Errors
    ///
    /// 非阻塞回收子进程状态失败时返回错误。
    pub(crate) fn take_notifications(&mut self) -> Result<Vec<String>, JobControlError> {
        self.reap_nonblocking()?;
        let mut lines = Vec::new();
        let mut completed = Vec::new();
        for (job_id, job) in &mut self.jobs {
            if !job.notified && matches!(job.state, JobState::Stopped | JobState::Done) {
                lines.push(job.display_line());
                job.notified = true;
                if job.state == JobState::Done {
                    completed.push(*job_id);
                }
            }
        }
        for job_id in completed {
            self.jobs.remove(&job_id);
        }
        Ok(lines)
    }

    /// 刷新并列出当前 JobTable 中的全部作业。
    ///
    /// # Returns
    ///
    /// 按 JobId 排列的作业展示行；本次列出的 `Done` 作业随后从表中移除。
    ///
    /// # Errors
    ///
    /// 非阻塞回收子进程状态失败时返回错误。
    pub(crate) fn list_jobs(&mut self) -> Result<Vec<String>, JobControlError> {
        self.reap_nonblocking()?;
        let lines = self.jobs.values().map(Job::display_line).collect();
        self.jobs.retain(|_, job| job.state != JobState::Done);
        Ok(lines)
    }

    /// 在后台继续一个已停止作业。
    ///
    /// # Arguments
    ///
    /// * `specification` - `%<job-id>`；省略时选择编号最大的已停止作业。
    ///
    /// # Returns
    ///
    /// 成功发送 `SIGCONT` 后的作业展示行。
    ///
    /// # Errors
    ///
    /// jobspec 无效、目标不存在或并未停止，或发送 `SIGCONT` 失败时返回错误。
    pub(crate) fn continue_background(
        &mut self,
        specification: Option<&str>,
    ) -> Result<String, JobControlError> {
        self.reap_nonblocking()?;
        let job_id = self.resolve_job_id(specification, true)?;
        let job = self
            .jobs
            .get_mut(&job_id)
            .ok_or(JobControlError::UnknownJob(job_id))?;
        if job.state != JobState::Stopped {
            return Err(JobControlError::JobNotStopped(job_id));
        }

        killpg(job.pgid, Signal::SIGCONT)
            .map_err(|source| JobControlError::SignalJob { job_id, source })?;
        job.mark_running();
        Ok(job.display_line())
    }

    /// 把指定作业转到前台，并等待它停止或完成。
    ///
    /// # Arguments
    ///
    /// * `specification` - `%<job-id>`；省略时选择编号最大的现存作业。
    ///
    /// # Returns
    ///
    /// 作业最后一个阶段的 Shell 状态码。
    ///
    /// # Errors
    ///
    /// jobspec 无效、目标不存在，或恢复、终端切换和等待过程中发生错误时返回错误。
    pub(crate) fn continue_foreground(
        &mut self,
        specification: Option<&str>,
    ) -> Result<i32, JobControlError> {
        self.reap_nonblocking()?;
        let job_id = self.resolve_job_id(specification, false)?;
        let stopped = self
            .jobs
            .get(&job_id)
            .is_some_and(|job| job.state == JobState::Stopped);
        self.wait_for_foreground_job(job_id, stopped)
    }

    /// 关闭 Job Control 会话并恢复 Shell 的终端状态。
    ///
    /// 交互模式会向未完成作业发送 `SIGHUP`，并继续已停止作业使其能够处理该信号；
    /// 非交互模式只执行一次非阻塞回收。清理错误在退出路径中被忽略。
    pub(crate) fn shutdown(&mut self) {
        if self.mode != JobControlMode::Interactive {
            let _ = self.reap_nonblocking();
            return;
        }

        let active: Vec<_> = self
            .jobs
            .values()
            .filter(|job| job.state != JobState::Done)
            .map(|job| (job.pgid, job.state))
            .collect();
        for (pgid, state) in active {
            let _ = killpg(pgid, Signal::SIGHUP);
            if state == JobState::Stopped {
                let _ = killpg(pgid, Signal::SIGCONT);
            }
        }
        let _ = self.reap_nonblocking();
        if let Some(session) = &self.session {
            let _ = tcsetpgrp(&session.terminal, session.shell_pgid);
            let _ = tcsetattr(
                &session.terminal,
                SetArg::TCSADRAIN,
                &session.shell_terminal_modes,
            );
        }
        self.mode = JobControlMode::NonInteractive;
        self.session = None;
    }

    /// 在启动失败路径中把控制终端立即交还给 Shell。
    ///
    /// 该清理操作是 best-effort；原始启动错误优先于恢复过程中产生的错误。
    pub(crate) fn restore_shell_terminal(&self) {
        if let Some(session) = &self.session {
            let _ = tcsetpgrp(&session.terminal, session.shell_pgid);
            let _ = tcsetattr(
                &session.terminal,
                SetArg::TCSADRAIN,
                &session.shell_terminal_modes,
            );
        }
    }

    /// 在需要时恢复作业运行，并把控制终端交给该作业。
    ///
    /// # Arguments
    ///
    /// * `job_id` - 目标作业编号。
    /// * `continue_stopped` - 是否先发送 `SIGCONT` 并更新进程状态。
    ///
    /// # Errors
    ///
    /// 作业不存在、发送信号或设置终端前台进程组/模式失败时返回错误。
    fn give_terminal_to_job(
        &mut self,
        job_id: JobId,
        continue_stopped: bool,
    ) -> Result<(), JobControlError> {
        let job = self
            .jobs
            .get_mut(&job_id)
            .ok_or(JobControlError::UnknownJob(job_id))?;

        if continue_stopped {
            // Continue before transferring the terminal. If SIGCONT fails, the Shell still
            // owns the terminal and can safely report the error and accept the next command.
            killpg(job.pgid, Signal::SIGCONT)
                .map_err(|source| JobControlError::SignalJob { job_id, source })?;
            job.mark_running();
        }

        if let Some(session) = &self.session {
            tcsetpgrp(&session.terminal, job.pgid).map_err(JobControlError::Terminal)?;
            if let Some(terminal_modes) = &job.terminal_modes {
                tcsetattr(&session.terminal, SetArg::TCSADRAIN, terminal_modes)
                    .map_err(JobControlError::Terminal)?;
            }
        }

        Ok(())
    }

    /// 按 PGID 等待作业，直到聚合状态不再是 `Running`。
    ///
    /// # Arguments
    ///
    /// * `job_id` - 要等待的已登记作业编号。
    ///
    /// # Returns
    ///
    /// 最后阶段的状态码；完成作业在返回前从 JobTable 移除。
    ///
    /// # Errors
    ///
    /// 作业不存在或 `waitpid` 失败时返回错误。
    fn wait_until_stopped_or_done(&mut self, job_id: JobId) -> Result<i32, JobControlError> {
        loop {
            let (pgid, state) = self
                .jobs
                .get(&job_id)
                .map(|job| (job.pgid, job.state))
                .ok_or(JobControlError::UnknownJob(job_id))?;
            if state != JobState::Running {
                break;
            }

            let flags = WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED;
            match waitpid(Pid::from_raw(-pgid.as_raw()), Some(flags)) {
                Ok(status) => self.apply_wait_status(status),
                Err(Errno::EINTR) => continue,
                Err(error) => return Err(JobControlError::Wait(error)),
            }
        }

        let job = self
            .jobs
            .get(&job_id)
            .ok_or(JobControlError::UnknownJob(job_id))?;
        let status = job.status_code();
        if job.state == JobState::Done {
            self.jobs.remove(&job_id);
        }
        Ok(status)
    }

    /// 保存已停止作业的终端模式，并把终端所有权和模式恢复给 Shell。
    ///
    /// # Arguments
    ///
    /// * `job_id` - 刚结束或停止的前台作业编号。
    ///
    /// # Errors
    ///
    /// 读取作业终端模式或恢复 Shell 的前台进程组/终端模式失败时返回错误。
    fn reclaim_terminal(&mut self, job_id: JobId) -> Result<(), JobControlError> {
        let Some(session) = &self.session else {
            return Ok(());
        };

        if let Some(job) = self.jobs.get_mut(&job_id)
            && job.state == JobState::Stopped
        {
            job.terminal_modes =
                Some(tcgetattr(&session.terminal).map_err(JobControlError::Terminal)?);
        }
        tcsetpgrp(&session.terminal, session.shell_pgid).map_err(JobControlError::Terminal)?;
        tcsetattr(
            &session.terminal,
            SetArg::TCSADRAIN,
            &session.shell_terminal_modes,
        )
        .map_err(JobControlError::Terminal)
    }

    /// 将可选 jobspec 解析为 JobTable 中的具体 JobId。
    ///
    /// # Arguments
    ///
    /// * `specification` - `%<job-id>` 或 `None`。
    /// * `stopped_only` - 默认选择时是否只考虑已停止作业。
    ///
    /// # Returns
    ///
    /// 显式指定或按 current-job 规则选出的 JobId。
    ///
    /// # Errors
    ///
    /// jobspec 格式无效、目标不存在或没有可作为默认值的作业时返回错误。
    fn resolve_job_id(
        &self,
        specification: Option<&str>,
        stopped_only: bool,
    ) -> Result<JobId, JobControlError> {
        if let Some(specification) = specification {
            let digits = specification
                .strip_prefix('%')
                .ok_or_else(|| JobControlError::InvalidJobSpec(specification.to_owned()))?;
            let id = digits
                .parse::<u32>()
                .map(JobId)
                .map_err(|_| JobControlError::InvalidJobSpec(specification.to_owned()))?;
            return self
                .jobs
                .contains_key(&id)
                .then_some(id)
                .ok_or(JobControlError::UnknownJob(id));
        }

        self.jobs
            .iter()
            .rev()
            .find(|(_, job)| !stopped_only || job.state == JobState::Stopped)
            .map(|(id, _)| *id)
            .ok_or(JobControlError::NoCurrentJob)
    }

    /// 把等待事件路由到拥有对应 PID 的作业。
    ///
    /// # Arguments
    ///
    /// * `status` - 某个已登记子进程的状态变化；未知 PID 会被忽略。
    fn apply_wait_status(&mut self, status: WaitStatus) {
        for job in self.jobs.values_mut() {
            if job.apply_wait_status(status) {
                return;
            }
        }
    }
}

impl Drop for JobControl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 为交互式父 Shell 安装应忽略的终端与中断信号。
///
/// # Errors
///
/// 任一 `sigaction` 调用失败时返回包含具体信号的错误。
fn install_shell_signal_dispositions() -> Result<(), JobControlError> {
    let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    for signal in SHELL_SIGNALS {
        // SAFETY: SIG_IGN is a kernel-defined disposition and does not call Rust code.
        unsafe { sigaction(signal, &ignore) }
            .map_err(|source| JobControlError::ConfigureSignal { signal, source })?;
    }
    Ok(())
}

/// 在 exec 或子 Shell 执行前恢复默认信号 disposition 并清空信号 mask。
///
/// # Errors
///
/// 任一 `sigaction` 或 `sigprocmask` 调用失败时返回对应的 `Errno`。
fn reset_child_signal_state() -> nix::Result<()> {
    let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    for signal in SHELL_SIGNALS.into_iter().chain([Signal::SIGCHLD]) {
        // SAFETY: SIG_DFL is a kernel-defined disposition and does not call Rust code.
        unsafe { sigaction(signal, &default)? };
    }
    sigprocmask(SigmaskHow::SIG_SETMASK, Some(&SigSet::empty()), None)
}

/// 提取等待事件关联的 PID。
///
/// # Arguments
///
/// * `status` - `waitpid` 返回的状态事件。
///
/// # Returns
///
/// 事件关联具体进程时返回 PID；`StillAlive` 返回 `None`。
fn wait_status_pid(status: &WaitStatus) -> Option<Pid> {
    match status {
        WaitStatus::Exited(pid, _)
        | WaitStatus::Signaled(pid, _, _)
        | WaitStatus::Stopped(pid, _)
        | WaitStatus::Continued(pid) => Some(*pid),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        WaitStatus::PtraceEvent(pid, _, _) | WaitStatus::PtraceSyscall(pid) => Some(*pid),
        WaitStatus::StillAlive => None,
    }
}

/// 将 `nix` 的 `Errno` 转换为 `pre_exec` 所需的 `io::Error`。
///
/// # Arguments
///
/// * `error` - 系统调用返回的 errno。
///
/// # Returns
///
/// 保留相同原始 OS 错误码的 I/O 错误。
fn errno_to_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(test)]
mod tests {
    use super::{Job, JobId, JobStage, JobState, RuntimeStage};
    use nix::{sys::signal::Signal, sys::wait::WaitStatus, unistd::Pid};

    #[test]
    fn job_is_stopped_only_after_every_live_process_stops() {
        let first = Pid::from_raw(101);
        let second = Pid::from_raw(102);
        let mut job = Job::new(
            JobId(1),
            first,
            "left | right".into(),
            vec![JobStage::Process(first), JobStage::Process(second)],
        );

        assert!(job.apply_wait_status(WaitStatus::Stopped(first, Signal::SIGTSTP)));
        assert_eq!(job.state, JobState::Running);
        assert!(job.apply_wait_status(WaitStatus::Stopped(second, Signal::SIGTSTP)));
        assert_eq!(job.state, JobState::Stopped);
    }

    #[test]
    fn pipeline_status_comes_from_the_last_stage() {
        let mut job = Job::new(
            JobId(1),
            Pid::from_raw(101),
            "left | right".into(),
            vec![JobStage::Completed(9), JobStage::Completed(3)],
        );

        job.recompute_state();
        assert_eq!(job.state, JobState::Done);
        assert_eq!(job.status_code(), 3);
        assert!(matches!(job.stages[0], RuntimeStage::Completed(9)));
    }
}
