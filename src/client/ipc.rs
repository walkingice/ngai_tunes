use crate::{
    daemon::runtime::{ROOT, SOCKET, verify_directory},
    protocol::{
        Command, IncomingState, Message, Request, VERSION, encode, encode_upload, read_frame,
    },
};
use std::{
    io::{self, BufReader, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::Path,
    process::{Command as Process, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

pub struct Connection {
    stream: UnixStream,
    receiver: Receiver<Message>,
    id: u64,
}

impl Connection {
    pub fn connect() -> io::Result<Self> {
        verify_directory(Path::new(ROOT))?;
        let stream = UnixStream::connect(SOCKET)?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let (send, receiver) = mpsc::sync_channel(16);
        thread::spawn(move || {
            let mut state = IncomingState::default();
            while let Ok(Some(line)) = read_frame(&mut reader) {
                let Ok(wire) = serde_json::from_slice(&line) else {
                    break;
                };
                let Ok(message) = state.receive(wire) else {
                    break;
                };
                if let Some(message) = message
                    && send.send(message).is_err()
                {
                    break;
                }
            }
        });
        let mut connection = Self {
            stream,
            receiver,
            id: 0,
        };
        connection.send(Command::Subscribe)?;
        Ok(connection)
    }

    pub fn start() -> io::Result<Self> {
        // The subscription snapshot can span many frames. Do not wait for it
        // before sending commands to an already bound daemon.
        if let Ok(connection) = Self::connect() {
            return Ok(connection);
        }
        let mut process = Process::new(std::env::current_exe()?);
        process
            .arg("--detached-child")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // setsid is async-signal-safe and detaches the child from the TUI.
        unsafe {
            process.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = process.spawn()?;
        thread::spawn(move || {
            let _ = child.wait();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(connection) = Self::connect() {
                return Ok(connection);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Daemon did not become ready",
                ));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn send(&mut self, command: Command) -> io::Result<u64> {
        self.id += 1;
        if let Command::ReplaceAndPlay { queue, selected } = command {
            for frame in encode_upload(self.id, &queue, selected)? {
                self.stream.write_all(&frame)?;
            }
        } else {
            let data = encode(&Request {
                version: VERSION,
                request_id: self.id,
                command,
            })?;
            self.stream.write_all(&data)?;
        }
        Ok(self.id)
    }

    pub fn next(&mut self) -> Result<Message, TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn finish(&mut self, command: Command) {
        let Ok(id) = self.send(command) else { return };
        let deadline = Instant::now() + Duration::from_millis(350);
        while let Ok(message) = self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if message.request_id == Some(id) || Instant::now() >= deadline {
                break;
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
