use crate::{
    daemon::runtime::{ROOT, SOCKET, verify_directory},
    protocol::{Command, Message, Request, VERSION, encode, read_frame},
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
    initial: Option<Message>,
}

impl Connection {
    pub fn connect() -> io::Result<Self> {
        verify_directory(Path::new(ROOT))?;
        let stream = UnixStream::connect(SOCKET)?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let (send, receiver) = mpsc::sync_channel(16);
        thread::spawn(move || {
            while let Ok(Some(line)) = read_frame(&mut reader) {
                let Ok(message) = serde_json::from_slice(&line) else {
                    break;
                };
                if send.send(message).is_err() {
                    break;
                }
            }
        });
        let mut connection = Self {
            stream,
            receiver,
            id: 0,
            initial: None,
        };
        connection.send(Command::Subscribe)?;
        Ok(connection)
    }

    fn ready(mut self, timeout: Duration) -> io::Result<Self> {
        let message = self
            .receiver
            .recv_timeout(timeout)
            .map_err(io::Error::other)?;
        if message.request_id != Some(1) || message.state.is_none() {
            return Err(io::Error::other(message.error.map_or_else(
                || "Invalid daemon handshake".into(),
                |error| error.message,
            )));
        }
        self.initial = Some(message);
        Ok(self)
    }

    pub fn start() -> io::Result<Self> {
        if let Ok(connection) =
            Self::connect().and_then(|peer| peer.ready(Duration::from_millis(250)))
        {
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
            if let Ok(connection) =
                Self::connect().and_then(|peer| peer.ready(Duration::from_millis(250)))
            {
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
        let data = encode(&Request {
            version: VERSION,
            request_id: self.id,
            command,
        })?;
        self.stream.write_all(&data)?;
        Ok(self.id)
    }

    pub fn next(&mut self) -> Result<Message, TryRecvError> {
        self.initial
            .take()
            .map_or_else(|| self.receiver.try_recv(), Ok)
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

    pub fn first(&self) -> io::Result<Message> {
        self.receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(io::Error::other)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
