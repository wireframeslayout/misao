//! Windows のプロセスツリー: 子を Job Object に入れ、`pane.close` では Job Object ごと終了させる。
//!
//! Job Object に入れたプロセスの子孫は自動的に同じ Job に入るので、孫プロセスも残らない
//! （`taskkill /T` のようにプロセス ID から木をたどる方式と違い、途中のプロセスが先に死んで木が切れても取りこぼさない）。
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` を付けるので、デーモンが落ちて Job のハンドルが閉じたときも子孫は残らない。
//! 割り当ては `CreateProcess` の後なので、起動直後の一瞬に子が作ったプロセスは Job に入らない
//! （シェルは起動に数 ms かかるので実用上は問題にならない。`CREATE_SUSPENDED` は portable-pty が出していない）。
#![allow(unsafe_code)]

use std::io;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};

use super::{Child, ExitInfo, GroupState};

/// Windows にはプロセスグループの pid 再利用の問題が無い（Job のハンドルで子孫を指す）ので、リーダーを保留しない。
pub const LEADER_HELD: bool = false;

/// 閉じるときに Job の残りのプロセスを終了させる Job Object のハンドル。
struct Job(HANDLE);

// SAFETY: Job Object のハンドルは、どのスレッドからでも使える（カーネルが排他する）。
unsafe impl Send for Job {}
// SAFETY: 同上。ハンドルの値は変更しない。
unsafe impl Sync for Job {}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: `self.0` は `CreateJobObjectW` が返した、まだ閉じていないハンドル。
        unsafe { CloseHandle(self.0) };
    }
}

impl Job {
    fn create() -> io::Result<Self> {
        // SAFETY: 属性と名前は NULL（名前なしの既定の Job を作る）。
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);
        // SAFETY: 全ビットがゼロの JOBOBJECT_EXTENDED_LIMIT_INFORMATION は有効な値（制限なし）。
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` はサイズを渡した有効な構造体で、呼び出しの間だけ借りる。
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: どちらも有効なハンドル。process は呼び出し側が生きている間だけ渡す。
        if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self) {
        // SAFETY: `self.0` は有効な Job のハンドル。
        unsafe { TerminateJobObject(self.0, 1) };
    }

    /// Job の中で動いているプロセスの数。
    fn active_processes(&self) -> io::Result<u32> {
        // SAFETY: 全ビットがゼロの構造体は有効な値で、成功したときだけ読む。
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` はサイズを渡した書き込める構造体。戻りの長さは要らないので NULL。
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut info).cast(),
                u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses)
    }
}

/// pty の子とその子孫を入れた Job Object。複製は同じ Job を指す。
#[derive(Clone)]
pub struct ProcessTree {
    job: Arc<Job>,
}

impl std::fmt::Debug for ProcessTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessTree")
    }
}

impl ProcessTree {
    /// 起動した子を新しい Job Object に入れる。入れられなければ失敗する（孫を止められない状態で続けない）。
    pub fn attach(child: &(dyn Child + Send + Sync), _pid: u32) -> Result<Self, String> {
        let process = child.as_raw_handle().ok_or("the child has no process handle")?;
        let job = Job::create().map_err(|e| format!("cannot create a job object: {e}"))?;
        job.assign(process as HANDLE)
            .map_err(|e| format!("cannot add the child to the job object: {e}"))?;
        Ok(Self { job: Arc::new(job) })
    }

    pub fn wait_exit(&self, mut child: Box<dyn Child + Send + Sync>) -> ExitInfo {
        match child.wait() {
            Ok(status) => ExitInfo {
                exit_code: i32::try_from(status.exit_code()).ok(),
                signal: None,
            },
            Err(_) => ExitInfo {
                exit_code: None,
                signal: None,
            },
        }
    }

    /// Windows にはシグナルが無いので、猶予なしで Job ごと強制終了する。
    pub fn hangup(&self) {
        self.job.terminate();
    }

    pub fn kill(&self) {
        self.job.terminate();
    }

    pub fn reap(&self) -> io::Result<()> {
        Ok(())
    }

    /// Job に動いているプロセスが残っているか（終了したリーダー自身は数えない）。
    pub fn group_state(&self) -> GroupState {
        match self.job.active_processes() {
            Ok(0) => GroupState::Empty,
            Ok(_) => GroupState::Alive,
            Err(_) => GroupState::Unknown,
        }
    }
}
