//! The executable's sole unsafe boundary: owned Win32 process/Job/ConPTY
//! lifetimes and exact inherited HANDLE values. No workload arguments are
//! interpreted here. All buffers and owners outlive the native calls using them.
use filedescriptor::{AsRawFileDescriptor, FileDescriptor};
use std::{
    cmp::Ordering,
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
    },
    process::Command,
    ptr::{null, null_mut},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal},
    System::{
        Console::{
            COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
            SetConsoleCtrlHandler,
        },
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

/// This runs before any handles are opened or worker threads started. The
/// caller transfers exclusive ownership of every validated incoming resource.
pub(crate) fn validate_inherited(number: usize) -> io::Result<()> {
    let mut flags = 0;
    // SAFETY: GetHandleInformation only validates the opaque numeric handle;
    // it cannot dereference it as application memory. `flags` is writable.
    checked(unsafe { GetHandleInformation(number as HANDLE, &mut flags) })
}

pub(crate) fn clear_inheritance(resource: &FileDescriptor) -> io::Result<()> {
    // SAFETY: the descriptor owns the live handle throughout this call.
    checked(unsafe {
        SetHandleInformation(resource.as_raw_file_descriptor(), HANDLE_FLAG_INHERIT, 0)
    })
}

struct Inheritance<'a> {
    resources: &'a [FileDescriptor],
    flags: Vec<u32>,
}
impl<'a> Inheritance<'a> {
    fn new(resources: &'a [FileDescriptor]) -> io::Result<Self> {
        let mut guard = Self {
            resources,
            flags: Vec::new(),
        };
        for resource in resources {
            let raw = resource.as_raw_file_descriptor();
            let mut flags = 0;
            // SAFETY: resources remain exclusively owned by the launch call;
            // every handle stays open until guard restoration and spawn finish.
            checked(unsafe { GetHandleInformation(raw, &mut flags) })?;
            checked(unsafe {
                SetHandleInformation(raw, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
            })?;
            guard.flags.push(flags);
        }
        Ok(guard)
    }
}
impl Drop for Inheritance<'_> {
    fn drop(&mut self) {
        for (resource, flags) in self.resources.iter().zip(&self.flags) {
            // SAFETY: borrowed resources outlive this guard, including errors.
            unsafe {
                SetHandleInformation(
                    resource.as_raw_file_descriptor(),
                    HANDLE_FLAG_INHERIT,
                    *flags & HANDLE_FLAG_INHERIT,
                );
            }
        }
    }
}

/// Fixed one-per-terminal closer. Legacy ClosePseudoConsole may block while
/// output drains, so it never executes on the supervisor or capture thread.
struct Console {
    handle: Arc<Mutex<Option<HPCON>>>,
    close: mpsc::SyncSender<()>,
}
impl Console {
    fn request_close(&self) {
        let _ = self.close.try_send(());
    }
    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        let handle = self
            .handle
            .lock()
            .map_err(|_| io::Error::other("console lock poisoned"))?;
        let raw =
            handle.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "console closed"))?;
        // SAFETY: the lock excludes the sole closer until resize returns.
        hresult(unsafe { ResizePseudoConsole(raw, dimensions(rows, cols)?) })
    }
}

pub(crate) struct Terminal {
    console: Arc<Console>,
    input: Option<File>,
    output: Option<File>,
    // Keep the console-side pipe endpoints until native child creation returns.
    startup_pipes: Option<(File, File)>,
}
impl Terminal {
    pub(crate) fn open(rows: u16, cols: u16) -> io::Result<Self> {
        let size = dimensions(rows, cols)?;
        // A detached launcher/new process group can inherit Ctrl-C suppression.
        // ConPTY children inherit that bit too, even in their separate console.
        // SAFETY: no callback or memory is passed; this resets the monitor's
        // inheritable attribute before creating its console and workload.
        checked(unsafe { SetConsoleCtrlHandler(None, 0) })?;
        let (input, writer) = pipe()?;
        let (reader, output) = pipe()?;
        let mut raw = 0;
        // SAFETY: synchronous pipe endpoints and output pointer are valid;
        // flags=0 avoids the cursor-position handshake. Creation owns HPCON.
        hresult(unsafe {
            CreatePseudoConsole(
                size,
                input.as_raw_handle(),
                output.as_raw_handle(),
                0,
                &mut raw,
            )
        })?;
        let handle = Arc::new(Mutex::new(Some(raw)));
        let closer = Arc::clone(&handle);
        let (close, requests) = mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("madrillo-console-close".into())
            .spawn(move || {
                let _ = requests.recv();
                let raw = closer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(raw) = raw {
                    // SAFETY: taking under the resize mutex grants unique ownership;
                    // no later operation can access HPCON. Output drains elsewhere.
                    unsafe {
                        ClosePseudoConsole(raw);
                    }
                }
            })
        {
            // No child has started. Closing our read side removes output
            // backpressure before the constructor releases its sole HPCON.
            drop(reader);
            drop(writer);
            // SAFETY: the closer never started; raw is still uniquely owned.
            unsafe {
                ClosePseudoConsole(raw);
            }
            return Err(error);
        }
        Ok(Self {
            console: Arc::new(Console { handle, close }),
            input: Some(writer),
            output: Some(reader),
            startup_pipes: Some((input, output)),
        })
    }
    pub(crate) fn reader(&mut self) -> io::Result<File> {
        self.output
            .take()
            .ok_or_else(|| io::Error::other("terminal output already consumed"))
    }
    pub(crate) fn writer(&mut self) -> io::Result<File> {
        self.input
            .take()
            .ok_or_else(|| io::Error::other("terminal input already consumed"))
    }
    pub(crate) fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        self.console.resize(rows, cols)
    }
}

pub(crate) struct Child {
    process: OwnedHandle,
    job: OwnedHandle,
    id: u32,
    console: Option<Arc<Console>>,
    pub(crate) stdout: Option<File>,
    pub(crate) stderr: Option<File>,
}
impl Child {
    pub(crate) fn spawn(
        command: &Command,
        mut terminal: Option<&mut Terminal>,
        inherited: Vec<FileDescriptor>,
        stdin: Option<File>,
        passthrough: bool,
    ) -> io::Result<Self> {
        let mut command_line = command_line(command)?;
        let environment = environment(command)?;
        let cwd = command
            .get_current_dir()
            .map(|path| wide(path.as_os_str()))
            .transpose()?;
        // SAFETY: null security/name request a private unnamed noninherited Job.
        let raw_job = unsafe { CreateJobObjectW(null(), null()) };
        if raw_job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful creation transfers a unique CloseHandle resource.
        let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
        if terminal.is_some() {
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the typed buffer matches this information class and stays
            // alive until the call returns. Pipe Jobs retain cooperative loss.
            checked(unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            })?;
        }
        let mut stdio = Vec::new();
        let (mut stdout, mut stderr) = (None, None);
        if terminal.is_none() {
            if passthrough {
                stdio.push(File::from(io::stdin().as_handle().try_clone_to_owned()?));
                stdio.push(File::from(io::stdout().as_handle().try_clone_to_owned()?));
                stdio.push(File::from(io::stderr().as_handle().try_clone_to_owned()?));
            } else {
                stdio.push(stdin.ok_or_else(|| io::Error::other("missing workload stdin"))?);
                let (reader, writer) = pipe()?;
                stdout = Some(reader);
                stdio.push(writer);
                let (reader, writer) = pipe()?;
                stderr = Some(reader);
                stdio.push(writer);
            }
            for file in &stdio {
                // SAFETY: these handles are privately owned launch resources;
                // all are closed after creation. Only the explicit list inherits.
                checked(unsafe {
                    SetHandleInformation(
                        file.as_raw_handle(),
                        HANDLE_FLAG_INHERIT,
                        HANDLE_FLAG_INHERIT,
                    )
                })?;
            }
        }
        let inheritance = Inheritance::new(&inherited)?;
        let handles: Vec<HANDLE> = stdio
            .iter()
            .map(AsRawHandle::as_raw_handle)
            .chain(
                inherited
                    .iter()
                    .map(AsRawFileDescriptor::as_raw_file_descriptor),
            )
            .collect();
        let jobs = [job.as_raw_handle()];
        let console = terminal.as_ref().map(|t| Arc::clone(&t.console));
        let console_lock = console
            .as_ref()
            .map(|c| {
                c.handle
                    .lock()
                    .map_err(|_| io::Error::other("console lock poisoned"))
            })
            .transpose()?;
        let mut attributes =
            Attributes::new(1 + u32::from(!handles.is_empty()) + u32::from(console.is_some()))?;
        // SAFETY: backing arrays and Job owner remain live and unmodified through
        // CreateProcessW. Membership is assigned atomically before child code.
        unsafe {
            attributes.set(
                PROC_THREAD_ATTRIBUTE_JOB_LIST,
                jobs.as_ptr().cast(),
                size_of_val(&jobs),
            )?;
        }
        if !handles.is_empty() {
            // SAFETY: all listed handles are live, inheritable and owned above.
            unsafe {
                attributes.set(
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                    handles.as_ptr().cast(),
                    size_of_val(handles.as_slice()),
                )?;
            }
        }
        if let Some(lock) = &console_lock {
            let raw = lock.ok_or_else(|| io::Error::other("console closed before launch"))?;
            // SAFETY: this attribute takes HPCON itself, not its address. The
            // held lock prevents close/resize until CreateProcessW returns.
            unsafe {
                attributes.set(
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                    raw as *const c_void,
                    size_of::<HPCON>(),
                )?;
            }
        }
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdio
            .first()
            .map_or(INVALID_HANDLE_VALUE, AsRawHandle::as_raw_handle);
        startup.StartupInfo.hStdOutput = stdio
            .get(1)
            .map_or(INVALID_HANDLE_VALUE, AsRawHandle::as_raw_handle);
        startup.StartupInfo.hStdError = stdio
            .get(2)
            .map_or(INVALID_HANDLE_VALUE, AsRawHandle::as_raw_handle);
        startup.lpAttributeList = attributes.pointer();
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: UTF-16 strings are NUL-terminated; command_line is writable.
        // All attribute backing and stdio/Job/console owners outlive this call.
        // The explicit handle list excludes control, hold, and capture resources.
        let created = unsafe {
            CreateProcessW(
                null(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                i32::from(!handles.is_empty()),
                CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                cwd.as_ref().map_or(null(), |text| text.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        };
        let error = (created == 0).then(io::Error::last_os_error);
        drop(attributes);
        drop(console_lock);
        drop(inheritance);
        if let Some(terminal) = &mut terminal {
            terminal.startup_pipes = None;
        }
        if let Some(error) = error {
            return Err(error);
        }
        // SAFETY: successful CreateProcessW transfers one process and one
        // thread handle. Neither is adopted twice; we immediately close thread.
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
        Ok(Self {
            process,
            job,
            id: info.dwProcessId,
            console,
            stdout,
            stderr,
        })
    }
    pub(crate) fn id(&self) -> u32 {
        self.id
    }
    pub(crate) fn try_wait(&self) -> io::Result<Option<u32>> {
        self.observe(0)
    }
    pub(crate) fn wait(&self) -> io::Result<u32> {
        self.observe(INFINITE)?
            .ok_or_else(|| io::Error::other("infinite process wait timed out"))
    }
    fn observe(&self, timeout: u32) -> io::Result<Option<u32>> {
        // SAFETY: the owned process handle remains valid throughout the wait.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), timeout) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: process is signaled; 259 is now a genuine exit code,
                // not evidence of a live process. Output storage is writable.
                checked(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
                Ok(Some(code))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub(crate) fn kill(&self) -> io::Result<()> {
        // SAFETY: this Job belongs only to the workload, never the monitor.
        checked(unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) })
    }
    pub(crate) fn wait_empty(&self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: the information class, exact buffer size and type agree;
            // the Job identity stays owned even after the leader has exited.
            checked(unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                )
            })?;
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "workload Job is still populated",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        if let Some(console) = &self.console {
            console.request_close();
        }
    }
}

struct Attributes {
    storage: Vec<u128>,
}
impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: null-list size query; bytes is valid writable output.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut list = Self {
            storage: vec![0; bytes.div_ceil(size_of::<u128>())],
        };
        // SAFETY: u128 storage is 16-byte-aligned and covers the queried size.
        if unsafe { InitializeProcThreadAttributeList(list.pointer(), count, 0, &mut bytes) } == 0 {
            // An unsuccessful initialization must not run Delete on the buffer.
            let error = io::Error::last_os_error();
            list.storage.clear();
            return Err(error);
        }
        Ok(list)
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    /// The backing value and every referenced native resource must outlive the
    /// attribute list and the process creation using it, without moving.
    unsafe fn set(&mut self, key: u32, value: *const c_void, bytes: usize) -> io::Result<()> {
        // SAFETY: the caller supplies stable backing of the attribute's type.
        checked(unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                key as usize,
                value,
                bytes,
                null_mut(),
                null(),
            )
        })
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if !self.storage.is_empty() {
            // SAFETY: initialization succeeded; storage remains live and unique.
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

fn pipe() -> io::Result<(File, File)> {
    let (reader, writer) = io::pipe()?;
    Ok((
        File::from(OwnedHandle::from(reader)),
        File::from(OwnedHandle::from(writer)),
    ))
}
fn dimensions(rows: u16, cols: u16) -> io::Result<COORD> {
    if rows == 0 || cols == 0 {
        return Err(io::Error::other("terminal dimensions must be nonzero"));
    }
    Ok(COORD {
        X: i16::try_from(cols).map_err(io::Error::other)?,
        Y: i16::try_from(rows).map_err(io::Error::other)?,
    })
}
fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn hresult(result: i32) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::other(format!("ConPTY HRESULT {result:#x}")))
    } else {
        Ok(())
    }
}
fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native text contains NUL",
        ));
    }
    value.push(0);
    Ok(value)
}
fn command_line(command: &Command) -> io::Result<Vec<u16>> {
    let program = wide(command.get_program())?;
    if program.len() == 1 || program.contains(&34) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid executable name",
        ));
    }
    let extension = std::path::Path::new(command.get_program())
        .extension()
        .and_then(OsStr::to_str);
    if extension.is_some_and(|s| s.eq_ignore_ascii_case("bat") || s.eq_ignore_ascii_case("cmd")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "batch scripts require an explicit command interpreter",
        ));
    }
    let mut result = Vec::new();
    for text in std::iter::once(command.get_program()).chain(command.get_args()) {
        if !result.is_empty() {
            result.push(32);
        }
        result.push(34);
        let mut slashes = 0;
        let value = wide(text)?;
        for c in &value[..value.len() - 1] {
            if *c == 92 {
                slashes += 1;
                continue;
            }
            result.extend(std::iter::repeat_n(
                92,
                if *c == 34 { slashes * 2 + 1 } else { slashes },
            ));
            result.push(*c);
            slashes = 0;
        }
        result.extend(std::iter::repeat_n(92, slashes * 2));
        result.push(34);
    }
    result.push(0);
    if result.len() > 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows command line too long",
        ));
    }
    Ok(result)
}
fn compare(left: &OsStr, right: &OsStr) -> io::Result<Ordering> {
    let left = wide(left)?;
    let right = wide(right)?;
    // SAFETY: slices remain live; lengths exclude NUL and fit the API's i32.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            i32::try_from(left.len() - 1).map_err(io::Error::other)?,
            right.as_ptr(),
            i32::try_from(right.len() - 1).map_err(io::Error::other)?,
            1,
        )
    };
    match result {
        CSTR_LESS_THAN => Ok(Ordering::Less),
        CSTR_EQUAL => Ok(Ordering::Equal),
        CSTR_GREATER_THAN => Ok(Ordering::Greater),
        _ => Err(io::Error::last_os_error()),
    }
}
fn environment(command: &Command) -> io::Result<Vec<u16>> {
    let mut entries: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        let native = wide(key)?;
        if native.len() == 1 || native.contains(&61) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid environment key",
            ));
        }
        let mut i = 0;
        while i < entries.len() {
            if compare(&entries[i].0, key)? == Ordering::Equal {
                entries.remove(i);
            } else {
                i += 1;
            }
        }
        if let Some(value) = value {
            entries.push((key.to_owned(), value.to_owned()));
        }
    }
    let mut error = None;
    entries.sort_by(|a, b| {
        compare(&a.0, &b.0).unwrap_or_else(|e| {
            error = Some(e);
            Ordering::Equal
        })
    });
    if let Some(error) = error {
        return Err(error);
    }
    let mut block = Vec::new();
    for (key, value) in entries {
        let mut text = key;
        text.push("=");
        text.push(value);
        block.extend(wide(&text)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct Monitor(Child);
    impl Drop for Monitor {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait_empty();
        }
    }

    #[test]
    fn exact_handles_compose_with_pipe_and_conpty_and_release_before_hook()
    -> Result<(), Box<dyn std::error::Error>> {
        let executable = std::env::current_exe()?;
        let binaries = executable
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or("missing Cargo binary directory")?;
        let fixture = binaries.join("madrillo-fixture.exe");
        for terminal in [false, true] {
            let root = tempfile::tempdir()?;
            let (mut reader, writer) = pipe()?;
            let number = writer.as_raw_handle() as usize;
            let hidden_path = root.path().join("hidden");
            std::fs::write(&hidden_path, b"monitor-only resource")?;
            let hidden = File::open(hidden_path)?;
            let hidden_number = hidden.as_raw_handle() as usize;
            let marker = root.path().join("hook");
            let mut command = Command::new(binaries.join("madrillo.exe"));
            command
                .arg("--state-dir")
                .arg(root.path().join("state"))
                .arg("--inherit")
                .arg(number.to_string())
                .arg("--hold")
                .arg(hidden_number.to_string())
                .arg("--exit-command")
                .arg(&fixture)
                .args(["--exit-command-arg", "touch-and-wait", "--exit-command-arg"])
                .arg(&marker);
            if terminal {
                command.arg("--terminal");
            }
            command
                .arg("--")
                .arg(&fixture)
                .arg("inherited-probe")
                .arg(number.to_string())
                .arg(hidden_number.to_string());
            let monitor = Monitor(Child::spawn(
                &command,
                None,
                vec![FileDescriptor::new(writer), FileDescriptor::new(hidden)],
                Some(File::open("NUL")?),
                false,
            )?);
            let (send, receive) = mpsc::sync_channel(1);
            let reader = std::thread::Builder::new()
                .name("exact-handle-reader".into())
                .spawn(move || {
                    let mut bytes = Vec::new();
                    let result = reader.read_to_end(&mut bytes).map(|_| bytes);
                    let _ = send.send(result);
                })?;
            // EOF must precede the delayed exit hook's completion: no monitor
            // writer copy or callback inheritance may keep this pipe open.
            assert_eq!(
                receive.recv_timeout(Duration::from_secs(8))??,
                b"opaque\0\xff"
            );
            reader.join().map_err(|_| "inheritance reader panicked")?;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !marker.exists() {
                assert_eq!(
                    monitor.0.try_wait()?,
                    None,
                    "monitor failed before its hook"
                );
                if Instant::now() >= deadline {
                    return Err("exit hook did not start".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                monitor.0.try_wait()?,
                None,
                "EOF arrived only after monitor exit"
            );
            let result: madrillo::Completion =
                serde_json::from_slice(&std::fs::read(root.path().join("state/result.json"))?)?;
            assert_eq!(
                result.leader,
                madrillo::LeaderResult::Exit { code: 37 },
                "{result:?}"
            );
            assert_eq!(
                result.cleanup,
                madrillo::CleanupResult::Complete,
                "{result:?}"
            );
            assert!(result.monitor_errors.is_empty(), "{result:?}");
            assert_eq!(monitor.0.wait()?, 0);
            monitor.0.wait_empty()?;
            println!(
                "exact Windows inheritance: terminal={terminal}, opaque bytes, excluded handle and EOF before hook passed"
            );
        }
        Ok(())
    }
}
