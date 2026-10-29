//! Authenticated, request-bound recovery; the parent never adopts a logger by name.
use super::*;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::{
    Foundation::{
        ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, GENERIC_READ, GENERIC_WRITE,
        INVALID_HANDLE_VALUE,
    },
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateFileW, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
        SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    },
    System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    },
};

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Receipt {
    Created { binding: String },
    Cleaned { binding: String },
}

fn binding(request: &ElevationRequest, sid: &str) -> Result<String, ElevationError> {
    let mut hash = Sha256::new();
    hash.update(b"pcagent-trace-receipt-v1\0");
    hash.update(serde_json::to_vec(request).map_err(|_| ElevationError::InvalidRequest)?);
    hash.update([0]);
    hash.update(sid.as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

fn pipe_name(binding: &str) -> Vec<u16> {
    wide(&format!(r"\\.\pipe\pcagent-trace-{binding}"))
}

fn write(pipe: HANDLE, bytes: &[u8]) -> Result<(), ElevationError> {
    let mut written = 0;
    // SAFETY: live pipe, bounded immutable bytes and initialized output.
    // Server handles use NOWAIT. Client writes only a single byte into an empty bounded buffer.
    if unsafe {
        WriteFile(
            pipe,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            null_mut(),
        )
    } == 0
        || written as usize != bytes.len()
    {
        return Err(ElevationError::InsecureJob);
    }
    Ok(())
}

fn send_receipt(pipe: HANDLE, receipt: &Receipt) -> Result<(), ElevationError> {
    let bytes = serde_json::to_vec(receipt).map_err(|_| ElevationError::InvalidResponse)?;
    let mut framed = (bytes.len() as u32).to_le_bytes().to_vec();
    framed.extend(bytes);
    write(pipe, &framed)
}

pub(super) struct Server {
    pipe: Handle,
    binding: String,
    connected: bool,
    announced: bool,
}

impl Server {
    /// Only called after Lease has acquired an OwnedSession from successful StartTrace.
    pub(super) fn after_creation(start: &Start, lease: &Lease) -> Result<Self, ElevationError> {
        if lease.session.is_none() {
            return Err(ElevationError::InsecureJob);
        }
        Self::open(start)
    }

    fn open(start: &Start) -> Result<Self, ElevationError> {
        let binding = binding(&start.request, &start.owner_sid)?;
        let name = pipe_name(&binding);
        let security = wide(&format!(
            "D:P(A;;GRGW;;;{})(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;ME)",
            start.owner_sid
        ));
        let mut descriptor = null_mut();
        // SAFETY: requester SID was validated by Lease; remaining SDDL is fixed.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                security.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: private local-only, non-inherited, bounded single-instance byte pipe.
        // NOWAIT keeps connection polling and writes inside the guardian's hard deadline.
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                MAX_MESSAGE as u32,
                MAX_MESSAGE as u32,
                0,
                &attributes,
            )
        };
        let error = last_error();
        // SAFETY: exclusively owned conversion allocation; CreateNamedPipe copied the descriptor.
        unsafe { LocalFree(descriptor) };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(error);
        }
        Ok(Self {
            pipe: Handle(pipe),
            binding,
            connected: false,
            announced: false,
        })
    }

    fn disconnect(&mut self) {
        // SAFETY: server-owned pipe; disconnection cannot affect another instance.
        unsafe { DisconnectNamedPipe(self.pipe.0) };
        self.connected = false;
        self.announced = false;
    }

    fn connect(&mut self) -> Result<bool, ElevationError> {
        if !self.connected {
            // SAFETY: nonblocking pipe; no asynchronous OVERLAPPED allocation is needed.
            if unsafe { ConnectNamedPipe(self.pipe.0, null_mut()) } != 0 {
                return Ok(false);
            }
            // SAFETY: immediately observes the preceding connection attempt.
            match unsafe { GetLastError() } {
                ERROR_PIPE_CONNECTED => self.connected = true,
                ERROR_PIPE_LISTENING => return Ok(false),
                ERROR_NO_DATA => {
                    self.disconnect();
                    return Ok(false);
                }
                _ => return Err(ElevationError::InsecureJob),
            }
        }
        Ok(true)
    }

    pub(super) fn stop_requested(&mut self) -> Result<bool, ElevationError> {
        if !self.connect()? {
            return Ok(false);
        }
        if !self.announced {
            send_receipt(
                self.pipe.0,
                &Receipt::Created {
                    binding: self.binding.clone(),
                },
            )?;
            self.announced = true;
        }
        match available(self.pipe.0)? {
            Some(0) => Ok(false),
            None => {
                self.disconnect();
                Ok(false)
            }
            Some(_) => {
                let mut command = [0];
                read_exact(self.pipe.0, &mut command, Instant::now() + POLL)?;
                if command == [1] {
                    Ok(true)
                } else {
                    self.disconnect();
                    Ok(false)
                }
            }
        }
    }

    pub(super) fn acknowledge_cleanup(&self) -> Result<(), ElevationError> {
        send_receipt(
            self.pipe.0,
            &Receipt::Cleaned {
                binding: self.binding.clone(),
            },
        )?;
        // Retain the pipe until the client has read the ack; never hold any ETW resource here.
        let mut ack = [0];
        read_exact(self.pipe.0, &mut ack, Instant::now() + CLEANUP_TIMEOUT)?;
        if ack != [2] {
            return Err(ElevationError::InsecureJob);
        }
        Ok(())
    }
}

fn authenticate_server(pipe: HANDLE) -> Result<Handle, ElevationError> {
    let mut pid = 0;
    // SAFETY: connected local pipe and initialized output; identity is supplied by the kernel.
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 || pid == 0 {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: kernel-reported identity; retaining this handle prevents PID-reuse confusion.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(last_error());
    }
    let process = Handle(process);
    let current = std::env::current_exe().map_err(|_| ElevationError::InsecureJob)?;
    #[cfg(not(test))]
    let expected = current.with_file_name(super::super::HELPER_FILE_NAME);
    #[cfg(test)]
    let expected = current;
    let expected: Vec<_> = expected.as_os_str().encode_wide().collect();
    if !privileged(process.0)? || image(process.0)? != expected {
        return Err(ElevationError::InsecureJob);
    }
    Ok(process)
}

pub(super) fn cleanup(
    request: &ElevationRequest,
    sid: &str,
) -> Result<super::super::trace_native::TraceCleanupStatus, ElevationError> {
    use super::super::trace_native::TraceCleanupStatus;
    // Validate identity syntax without rejecting expiry: cleanup must also work after the deadline.
    Identity::request(request)?;
    let expected = binding(request, sid)?;
    let name = pipe_name(&expected);
    // SAFETY: fixed local endpoint, no inheritance, no creation and no impersonation authority.
    let pipe = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            null_mut(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            null_mut(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        return Ok(TraceCleanupStatus::Unvalidated);
    }
    let pipe = Handle(pipe);
    let _server = authenticate_server(pipe.0)?;
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    match receive::<Receipt>(pipe.0, deadline)? {
        Receipt::Created { binding } if binding == expected => {}
        _ => return Err(ElevationError::InsecureJob),
    }
    write(pipe.0, &[1])?;
    match receive::<Receipt>(pipe.0, deadline)? {
        Receipt::Cleaned { binding } if binding == expected => {}
        _ => return Err(ElevationError::InsecureJob),
    }
    write(pipe.0, &[2])?;
    Ok(TraceCleanupStatus::Stopped)
}

#[cfg(test)]
pub(super) fn forged_acceptance(start: &Start) {
    let request = start.request.clone();
    let sid = start.owner_sid.clone();
    let start = Start {
        request: request.clone(),
        owner_sid: sid.clone(),
    };
    let (ready, receive_ready) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        // Test-only spoof from the exact expected image; kernel authentication alone is not enough.
        let mut server = Server::open(&start).unwrap();
        ready.send(()).unwrap();
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while !server.connect().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(POLL);
        }
        send_receipt(
            server.pipe.0,
            &Receipt::Created {
                binding: "wrong-request".into(),
            },
        )
        .unwrap();
        while Instant::now() < deadline {
            match available(server.pipe.0).unwrap() {
                None => return,
                Some(bytes) => assert_eq!(bytes, 0, "forged receipt must never receive a STOP"),
            }
            std::thread::sleep(POLL);
        }
        panic!("forged receipt client did not disconnect");
    });
    receive_ready.recv_timeout(CLEANUP_TIMEOUT).unwrap();
    assert!(matches!(
        cleanup(&request, &sid),
        Err(ElevationError::InsecureJob)
    ));
    server.join().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_binding_covers_request_and_requester() {
        let mut request = ElevationRequest::new(
            ElevatedTask::PerformanceTrace {
                duration_seconds: 30,
            },
            Duration::from_secs(40),
        )
        .unwrap();
        let first = binding(&request, "S-1-5-21-1-2-3-1001").unwrap();
        assert_ne!(first, binding(&request, "S-1-5-21-1-2-3-1002").unwrap());
        request.task = ElevatedTask::PerformanceTrace {
            duration_seconds: 60,
        };
        assert_ne!(first, binding(&request, "S-1-5-21-1-2-3-1001").unwrap());
        assert!(!String::from_utf16(&pipe_name(&first))
            .unwrap()
            .contains(&request.nonce));
    }

    #[test]
    fn receipt_protocol_rejects_unknown_fields_and_states() {
        for text in [
            r#"{"state":"created","binding":"x","command":"stop"}"#,
            r#"{"state":"ready","binding":"x"}"#,
            r#"{"state":"cleaned"}"#,
        ] {
            assert!(serde_json::from_str::<Receipt>(text).is_err());
        }
    }
}
