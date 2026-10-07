//! Spawning and talking to helper processes (audian-stt, audian-llm, agy).
//!
//! Every child is placed in a Job Object configured to kill its members when the job handle
//! closes, so helpers can never outlive the daemon, even if it crashes. Responses are read on
//! a dedicated thread so callers can wait with a timeout instead of blocking forever on a pipe.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use audian_common::ipc;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    #[error("could not start {0}: {1}")]
    Spawn(String, io::Error),
    #[error("{0} stopped unexpectedly")]
    Died(String),
    #[error("{0} did not respond in time")]
    Timeout(String),
    #[error("communication with {0} failed: {1}")]
    Io(String, io::Error),
}

/// Kill-on-close job shared by all helper processes (handle intentionally never closed:
/// it is released by the OS when the daemon exits, which kills the helpers).
fn job() -> Option<HANDLE> {
    static JOB: OnceLock<Option<isize>> = OnceLock::new();
    JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .ok()?;
        Some(job.0 as isize)
    })
    .map(|h| HANDLE(h as *mut _))
}

pub fn adopt(child: &Child) {
    if let Some(job) = job() {
        unsafe {
            if let Err(e) = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) {
                log::warn!("could not add helper to job object: {e}");
            }
        }
    }
}

/// A framed-protocol helper (audian-stt / audian-llm).
pub struct FramedHelper {
    name: String,
    child: Child,
    stdin: BufWriter<ChildStdin>,
    frames: Receiver<io::Result<(u8, Vec<u8>)>>,
}

impl FramedHelper {
    pub fn spawn(exe: &Path, args: &[String]) -> Result<FramedHelper, HelperError> {
        let name = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| HelperError::Spawn(name.clone(), e))?;
        adopt(&child);
        let stdin = BufWriter::new(child.stdin.take().expect("piped stdin"));
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("{name}-reader"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let frame = ipc::read_frame(&mut reader);
                    let failed = frame.is_err();
                    if tx.send(frame).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|e| HelperError::Spawn(name.clone(), e))?;
        log::info!("started {name} (pid {})", child.id());
        Ok(FramedHelper { name, child, stdin, frames: rx })
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), HelperError> {
        ipc::send_json(&mut self.stdin, msg).map_err(|e| self.io_error(e))
    }

    pub fn send_samples(&mut self, samples: &[f32]) -> Result<(), HelperError> {
        ipc::send_samples(&mut self.stdin, samples).map_err(|e| self.io_error(e))
    }

    pub fn recv<T: DeserializeOwned>(&mut self, timeout: Duration) -> Result<T, HelperError> {
        match self.frames.recv_timeout(timeout) {
            Ok(Ok((kind, payload))) if kind == ipc::KIND_JSON => {
                ipc::parse_json(&payload).map_err(|e| HelperError::Io(self.name.clone(), e))
            }
            Ok(Ok(_)) => Err(HelperError::Io(self.name.clone(), io::Error::other("unexpected frame"))),
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(HelperError::Died(self.name.clone())),
            Err(RecvTimeoutError::Timeout) => Err(HelperError::Timeout(self.name.clone())),
        }
    }

    fn io_error(&mut self, e: io::Error) -> HelperError {
        if self.is_alive() { HelperError::Io(self.name.clone(), e) } else { HelperError::Died(self.name.clone()) }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for FramedHelper {
    fn drop(&mut self) {
        // Closing stdin lets the helper exit cleanly; kill as a backstop.
        let _ = self.stdin.flush();
        self.kill();
    }
}

/// Reads newline-delimited text from a pipe on a background thread.
pub fn spawn_line_reader<R: Read + Send + 'static>(name: &str, pipe: R) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new().name(format!("{name}-lines")).spawn(move || {
        use std::io::BufRead;
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}
