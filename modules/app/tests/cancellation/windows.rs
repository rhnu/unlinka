use std::{
    ffi::OsString,
    fs::{self, File},
    io,
    mem::size_of,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Path,
    ptr::null,
    thread,
    time::{Duration, Instant},
};

use unlinka_validation::{Fixture, counts};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_TIMEOUT},
    System::{
        Console::{AttachConsole, CTRL_BREAK_EVENT, FreeConsole, GenerateConsoleCtrlEvent},
        Threading::{
            CREATE_NEW_CONSOLE, CREATE_NEW_PROCESS_GROUP, CreateProcessW, GetExitCodeProcess,
            OpenProcess, PROCESS_INFORMATION, PROCESS_SYNCHRONIZE, STARTF_USESHOWWINDOW,
            STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
};

pub fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--signal-helper") {
        let pid = args
            .get(1)
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| io::Error::other("helper requires child process ID"))?;
        return signal(pid);
    }
    if !args
        .iter()
        .any(|arg| arg == "--ignored" || arg == "--include-ignored")
    {
        println!(
            "SKIPPED cancellation: pass --ignored to create/delete fixtures and send isolated CTRL_BREAK_EVENT"
        );
        return Ok(());
    }
    let fixture = Fixture::new("cancellation")?;
    let binary = fixture.path("unlinka.exe")?;
    fs::copy(env!("CARGO_BIN_EXE_unlinka"), &binary)?;
    let sentinel = fixture.path("outside-target-sentinel.txt")?;
    let content = b"This file is outside every deletion target.";
    fs::write(&sentinel, content)?;
    for threads in [1, 8] {
        cancellation_case(&fixture, &binary, &sentinel, content, threads)?;
    }
    Ok(())
}

fn cancellation_case(
    fixture: &Fixture,
    binary: &Path,
    sentinel: &Path,
    content: &[u8],
    threads: usize,
) -> io::Result<()> {
    const FILES: usize = 6000;
    let case = fixture.folder(format!("threads-{threads}"))?;
    let target = fixture.folder(format!("threads-{threads}/target"))?;
    for i in 0..FILES {
        fs::write(target.join(format!("file-{i:05}.txt")), [1, 2, 3, 4])?;
    }
    let error_file = case.join("stderr.txt");
    let child = Child::spawn(binary, &target, &case, threads)?;
    let ready = Instant::now();
    let mut observed = false;
    while ready.elapsed() < Duration::from_secs(10) && child.running()? {
        let text = fs::read_to_string(&error_file)?;
        if text.contains("Starting deletion")
            && (0..64).any(|i| !target.join(format!("file-{i:05}.txt")).exists())
        {
            observed = true;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    if !observed || !child.running()? {
        return Err(io::Error::other(format!(
            "threads={threads}: process finished before a deletion could be interrupted"
        )));
    }
    let started = Instant::now();
    let helper_args = [
        OsString::from("--signal-helper"),
        child.pid.to_string().into(),
    ];
    let helper = unlinka_validation::run(&std::env::current_exe()?, &helper_args, &case)?;
    if !helper.status.success() {
        return Err(io::Error::other(format!(
            "isolated signal helper failed: {}",
            String::from_utf8_lossy(&helper.stderr)
        )));
    }
    while child.running()? && started.elapsed() < Duration::from_secs(10) {
        thread::sleep(Duration::from_millis(1));
    }
    if child.running()? {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "cancelled child exceeded 10 seconds",
        ));
    }
    let exit = child.exit_code()?;
    let text = fs::read_to_string(&error_file)?;
    let report = counts(&text)?;
    let remaining = if target.exists() {
        fs::read_dir(&target)?
            .collect::<io::Result<Vec<_>>>()?
            .len()
    } else {
        0
    };
    if exit != 3
        || !text.contains("Deletion cancelled")
        || remaining == 0
        || remaining >= FILES
        || report.files != u64::try_from(FILES - remaining).expect("6000 file count fits a u64")
        || report.failed != 0
        || report.directories != 0
        || report.links != 0
        || fs::read(sentinel)? != content
    {
        return Err(io::Error::other(format!(
            "threads={threads}: exit={exit}, remaining={remaining}, counts={report:?}\n{text}"
        )));
    }
    let timing = String::from_utf8(helper.stdout).map_err(io::Error::other)?;
    println!(
        "Cancellation with {threads} thread(s): passed; {remaining}/{FILES} remain; {} deleted; 0 failures; {}; sentinel unchanged",
        report.files,
        timing.trim()
    );
    Ok(())
}

fn signal(pid: u32) -> io::Result<()> {
    // SAFETY: this helper is an independent process. Detaching its inherited
    // console cannot change the caller's console; the target PID identifies the
    // new console/process group created below, never the user's process group.
    unsafe {
        FreeConsole();
    }
    // SAFETY: pid is the independently launched deletion child's console.
    if unsafe { AttachConsole(pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: opens only a synchronization handle for the owned child PID.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: detach only this helper process from the child console.
        unsafe {
            FreeConsole();
        }
        return Err(error);
    }
    let started = Instant::now();
    // SAFETY: the event targets only the child's new process group.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
    let result = if sent == 0 {
        Err(io::Error::last_os_error())
    // SAFETY: process is a live handle retained until after the bounded wait.
    } else if unsafe { WaitForSingleObject(process, 10000) } != 0 {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "child did not exit within 10 seconds of CTRL_BREAK_EVENT",
        ))
    } else {
        println!(
            "{:.3}s from signal to exit",
            started.elapsed().as_secs_f64()
        );
        Ok(())
    };
    // SAFETY: closes the independently owned synchronization handle once.
    unsafe {
        CloseHandle(process);
    }
    // SAFETY: detach only this helper process from the child console.
    unsafe {
        FreeConsole();
    }
    result
}

struct Child {
    handle: HANDLE,
    pid: u32,
}

impl Child {
    fn spawn(binary: &Path, target: &Path, cwd: &Path, threads: usize) -> io::Result<Self> {
        let input = File::open("NUL")?;
        let output = File::create(cwd.join("stdout.txt"))?;
        let error = File::create(cwd.join("stderr.txt"))?;
        let handles = [
            input.as_raw_handle(),
            output.as_raw_handle(),
            error.as_raw_handle(),
        ];
        for handle in handles {
            // SAFETY: these three handles are locally owned live files. No other
            // process creation runs concurrently in this custom test harness.
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
                == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        let binary_name: Vec<_> = binary.as_os_str().encode_wide().chain([0]).collect();
        let cwd_name: Vec<_> = cwd.as_os_str().encode_wide().chain([0]).collect();
        // Windows paths cannot contain quotes; arguments end in .exe/target,
        // so quoted path arguments have no trailing backslash ambiguity.
        let mut command: Vec<_> = format!(
            "\"{}\" -r -y --threads {threads} -- \"{}\"",
            binary.display(),
            target.display()
        )
        .encode_utf16()
        .chain([0])
        .collect();
        let startup = STARTUPINFOW {
            cb: u32::try_from(size_of::<STARTUPINFOW>()).expect("STARTUPINFOW size fits a DWORD"),
            dwFlags: STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES,
            wShowWindow: 0,
            hStdInput: handles[0],
            hStdOutput: handles[1],
            hStdError: handles[2],
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: all strings are terminated and live through this synchronous
        // call; structures match the ABI, inherited standard handles are live.
        let created = unsafe {
            CreateProcessW(
                binary_name.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP,
                null(),
                cwd_name.as_ptr(),
                &raw const startup,
                &raw mut process,
            )
        };
        let failure = io::Error::last_os_error();
        for handle in handles {
            // SAFETY: all file owners remain live; inheritance is no longer needed.
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
        if created == 0 {
            return Err(failure);
        }
        // SAFETY: CreateProcess returned this independently owned thread handle.
        unsafe {
            CloseHandle(process.hThread);
        }
        Ok(Self {
            handle: process.hProcess,
            pid: process.dwProcessId,
        })
    }

    fn running(&self) -> io::Result<bool> {
        // SAFETY: self owns a live process handle throughout the call.
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_TIMEOUT => Ok(true),
            0 => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn exit_code(&self) -> io::Result<u32> {
        let mut code = 0;
        // SAFETY: the owned process handle and writable DWORD remain live.
        if unsafe { GetExitCodeProcess(self.handle, &raw mut code) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(code)
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: this test uniquely owns the child process. On every early
        // return, kill and reap it before the fixture owner removes its tree.
        if unsafe { WaitForSingleObject(self.handle, 0) } == WAIT_TIMEOUT {
            // SAFETY: this handle refers only to the independently owned child.
            unsafe {
                TerminateProcess(self.handle, 1);
            }
            // SAFETY: the owned handle stays live for this bounded wait.
            unsafe {
                WaitForSingleObject(self.handle, 10000);
            }
        }
        // SAFETY: this is the sole close of the valid owned process handle.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
