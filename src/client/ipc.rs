use crate::{
    daemon::runtime::{ROOT, SOCKET, verify_directory},
    protocol::{
        Command, IncomingState, Message, Request, VERSION, WireMessage, encode, encode_upload,
        read_frame,
    },
};
use crossbeam_channel::{self as channel, Receiver};
use std::{
    io::{self, BufReader, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::Path,
    process::{Command as Process, Stdio},
    sync::mpsc::TryRecvError,
    thread,
    time::{Duration, Instant},
};

pub struct Transport<T> {
    stream: UnixStream,
    pub(super) receiver: Receiver<io::Result<T>>,
    id: u64,
}

pub type Connection = Transport<Message>;

impl Connection {
    pub fn connect() -> io::Result<Self> {
        let mut state = IncomingState::default();
        Self::subscribe(Command::Subscribe, move |wire| {
            state.receive(wire).map_err(io::Error::other)
        })
    }

    pub fn start() -> io::Result<Self> {
        start(Self::connect)
    }

    pub fn next(&mut self) -> Result<Message, TryRecvError> {
        match self.receiver.try_recv() {
            Ok(Ok(message)) => Ok(message),
            Err(channel::TryRecvError::Empty) => Err(TryRecvError::Empty),
            _ => Err(TryRecvError::Disconnected),
        }
    }

    pub fn finish(&mut self, command: Command) {
        let Ok(id) = self.send(command) else { return };
        let deadline = Instant::now() + Duration::from_millis(350);
        while let Ok(Ok(message)) = self.receiver.recv_deadline(deadline) {
            if message.request_id == Some(id) || Instant::now() >= deadline {
                break;
            }
        }
    }
}

pub(super) fn open_socket() -> io::Result<UnixStream> {
    verify_directory(Path::new(ROOT))?;
    let stream = UnixStream::connect(SOCKET)?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    Ok(stream)
}

pub(super) fn is_offline(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

pub(super) fn start<T>(connect: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    start_with(connect, launch)
}

pub(super) fn start_with<T>(
    mut connect: impl FnMut() -> io::Result<T>,
    launch: impl FnOnce() -> io::Result<()>,
) -> io::Result<T> {
    match connect() {
        Err(error) if is_offline(&error) => {}
        result => return result,
    }
    launch()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match connect() {
            Err(error) if is_offline(&error) => {}
            result => return result,
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Server did not become ready",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn launch() -> io::Result<()> {
    let mut process = Process::new(std::env::current_exe()?);
    process
        .arg("--detached-child")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // setsid is async-signal-safe and detaches the child from the client.
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
    Ok(())
}

impl<T: Send + 'static> Transport<T> {
    pub(super) fn subscribe(
        command: Command,
        decode: impl FnMut(WireMessage) -> io::Result<Option<T>> + Send + 'static,
    ) -> io::Result<Self> {
        let mut connection = Self::open(decode)?;
        connection.send(command)?;
        Ok(connection)
    }

    pub(super) fn open(
        decode: impl FnMut(WireMessage) -> io::Result<Option<T>> + Send + 'static,
    ) -> io::Result<Self> {
        Self::from_stream(open_socket()?, decode)
    }

    pub(super) fn from_stream(
        stream: UnixStream,
        mut decode: impl FnMut(WireMessage) -> io::Result<Option<T>> + Send + 'static,
    ) -> io::Result<Self> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let (send, receiver) = channel::bounded(16);
        thread::spawn(move || {
            let result = (|| -> io::Result<()> {
                while let Some(line) = read_frame(&mut reader)? {
                    if let Some(message) = decode(serde_json::from_slice(&line)?)?
                        && send.send(Ok(message)).is_err()
                    {
                        break;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = send.send(Err(error));
            }
        });
        Ok(Self {
            stream,
            receiver,
            id: 0,
        })
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
}

impl<T> Drop for Transport<T> {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn play_probes_before_launching_and_waits_for_readiness() {
        for appeared in [false, true] {
            let probes = Cell::new(0);
            let launched = Cell::new(0);
            let ready = start_with(
                || {
                    probes.set(probes.get() + 1);
                    if appeared || launched.get() > 0 {
                        Ok("ready")
                    } else {
                        Err(io::ErrorKind::NotFound.into())
                    }
                },
                || {
                    launched.set(launched.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(ready, "ready");
            assert_eq!(launched.get(), usize::from(!appeared));
            assert_eq!(probes.get(), 1 + launched.get());
        }
    }

    #[test]
    fn protocol_and_permission_failures_do_not_launch() {
        for kind in [io::ErrorKind::InvalidData, io::ErrorKind::PermissionDenied] {
            let result = start_with::<()>(|| Err(kind.into()), || panic!("unexpected launch"));
            assert_eq!(result.unwrap_err().kind(), kind);
        }
    }
}
