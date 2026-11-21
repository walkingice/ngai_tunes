mod controller;
pub mod runtime;

use crate::{
    playback::{Audio, Engine},
    protocol::{Command, Failure, Message, Request, Snapshot, VERSION, encode, read_frame},
};
use controller::Controller;
use runtime::Runtime;
use std::{
    collections::HashMap,
    io::{self, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc,
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

type Frame = Arc<[u8]>;

struct Output {
    frame: Frame,
    close: bool,
    delivered: Option<mpsc::Sender<()>>,
}

impl Output {
    fn normal(frame: Frame) -> Self {
        Self {
            frame,
            close: false,
            delivered: None,
        }
    }
}

enum Input {
    Request(u64, Request),
    Closed(u64),
}

struct Peer {
    output: SyncSender<Output>,
    stream: UnixStream,
    subscribed: bool,
    graceful: bool,
}

impl Drop for Peer {
    fn drop(&mut self) {
        if !self.graceful {
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    serve(Runtime::bind(Path::new(runtime::ROOT))?, Audio::default())?;
    Ok(())
}

fn serve(runtime: Runtime, engine: impl Engine) -> io::Result<()> {
    let (input, requests) = mpsc::sync_channel(64);
    let mut peers = HashMap::new();
    let mut id = 0;
    let mut controller = Controller::new(engine);
    while !controller.shutdown {
        for _ in 0..16 {
            match runtime.listener.accept() {
                Ok((stream, _)) if peers.len() < 32 => {
                    id += 1;
                    if let Ok(peer) = connect_peer(id, stream, input.clone()) {
                        peers.insert(id, peer);
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        let revision = controller.revision();
        match requests.recv_timeout(Duration::from_millis(25)) {
            Ok(Input::Request(id, request)) => respond(&mut controller, &mut peers, id, request),
            Ok(Input::Closed(id)) => {
                peers.remove(&id);
            }
            Err(_) => {}
        }
        if controller.shutdown {
            break;
        }
        controller.tick();
        if revision != controller.revision() {
            let state = message(None, Some(controller.snapshot()), None);
            if let Ok(frame) = encode(&state) {
                let frame: Frame = frame.into();
                peers.retain(|_, peer| {
                    !peer.subscribed || peer.output.try_send(Output::normal(frame.clone())).is_ok()
                });
            }
        }
    }
    Ok(())
}

fn message(id: Option<u64>, state: Option<Snapshot>, error: Option<(&str, &str)>) -> Message {
    Message {
        request_id: id,
        state,
        error: error.map(|(code, message)| Failure {
            code: code.into(),
            message: message.into(),
        }),
    }
}

fn respond(
    controller: &mut Controller<impl Engine>,
    peers: &mut HashMap<u64, Peer>,
    id: u64,
    request: Request,
) {
    let Some(peer) = peers.get_mut(&id) else {
        return;
    };
    let request_id = Some(request.request_id);
    let response = if request.version != VERSION {
        message(
            request_id,
            None,
            Some(("version", "Unsupported protocol version")),
        )
    } else if let Command::ReplaceAndPlay { queue, .. } = &request.command
        && serde_json::to_vec(queue)
            .is_ok_and(|queue| queue.len() > crate::protocol::MAX_FRAME - 8192)
    {
        message(request_id, None, Some(("frame_size", "Queue is too large")))
    } else {
        let subscribe = matches!(request.command, Command::Subscribe);
        let snapshot = subscribe || matches!(request.command, Command::GetState);
        match controller.command(request.command) {
            Ok(()) => {
                peer.subscribed |= subscribe;
                message(request_id, snapshot.then(|| controller.snapshot()), None)
            }
            Err(error) => message(request_id, None, Some(("invalid_command", error))),
        }
    };
    let (delivered, wait) = mpsc::channel();
    let closing = controller.shutdown;
    if encode(&response).map_or(true, |frame| {
        peer.output
            .try_send(Output {
                frame: frame.into(),
                close: closing,
                delivered: closing.then_some(delivered),
            })
            .is_err()
    }) {
        peers.remove(&id);
    } else if closing {
        peer.graceful = wait.recv_timeout(Duration::from_millis(100)).is_ok();
    }
}

fn connect_peer(id: u64, stream: UnixStream, input: SyncSender<Input>) -> io::Result<Peer> {
    let mut writer = stream.try_clone()?;
    writer.set_write_timeout(Some(Duration::from_secs(1)))?;
    let reader = stream.try_clone()?;
    let (output, frames) = mpsc::sync_channel::<Output>(8);
    let errors = output.clone();
    let writer_input = input.clone();
    thread::spawn(move || {
        for output in frames {
            if writer.write_all(&output.frame).is_err() {
                let _ = writer.shutdown(std::net::Shutdown::Both);
                break;
            }
            if output.close {
                let _ = writer.shutdown(std::net::Shutdown::Write);
            }
            if let Some(delivered) = output.delivered {
                let _ = delivered.send(());
            }
            if output.close {
                break;
            }
        }
        let _ = writer_input.send(Input::Closed(id));
    });
    thread::spawn(move || read_requests(id, reader, input, errors));
    Ok(Peer {
        output,
        stream,
        subscribed: false,
        graceful: false,
    })
}

fn read_requests(
    id: u64,
    stream: UnixStream,
    input: SyncSender<Input>,
    errors: SyncSender<Output>,
) {
    let mut reader = BufReader::new(stream);
    loop {
        let (code, description, close) = match read_frame(&mut reader) {
            Ok(Some(frame)) => match serde_json::from_slice::<Request>(&frame) {
                Ok(request) => {
                    if input.send(Input::Request(id, request)).is_err() {
                        break;
                    }
                    continue;
                }
                Err(_) => ("malformed", "Invalid JSON request", false),
            },
            Ok(None) => break,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                ("frame_size", "Frame is too large", true)
            }
            Err(_) => break,
        };
        if let Ok(frame) = encode(&message(None, None, Some((code, description))))
            && errors
                .try_send(Output {
                    frame: frame.into(),
                    close,
                    delivered: None,
                })
                .is_err()
        {
            break;
        }
        if close {
            // Read no more data. The writer can deliver the error before peer cleanup.
            let _ = reader.get_ref().shutdown(std::net::Shutdown::Read);
            return;
        }
    }
    let _ = input.send(Input::Closed(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        playback::{Event, StartError},
        protocol::{Order, Repeat, Track},
    };

    #[derive(Default)]
    struct Silent;

    impl Engine for Silent {
        fn start(&mut self, _: &Path, _: u64) -> Result<Option<Duration>, StartError> {
            Ok(Some(Duration::from_secs(30)))
        }
        fn pause(&mut self, _: bool) {}
        fn stop(&mut self) {}
        fn seek(&mut self, _: Duration) -> Result<(), String> {
            Ok(())
        }
        fn position(&self) -> Duration {
            Duration::ZERO
        }
        fn event(&mut self) -> Option<Event> {
            None
        }
    }

    fn request(peer: &mut BufReader<UnixStream>, id: u64, command: Command) -> Message {
        let frame = encode(&Request {
            version: VERSION,
            request_id: id,
            command,
        })
        .unwrap();
        for chunk in frame.chunks(7) {
            peer.get_mut().write_all(chunk).unwrap();
        }
        loop {
            let frame = read_frame(peer)
                .unwrap()
                .unwrap_or_else(|| panic!("EOF waiting for request {id}"));
            let message: Message = serde_json::from_slice(&frame).unwrap();
            if message.request_id == Some(id) {
                assert!(message.error.is_none());
                return message;
            }
        }
    }

    #[test]
    fn two_clients_share_state_and_quit_is_acknowledged() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("run");
        let runtime = Runtime::bind(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let server = thread::spawn(move || serve(runtime, Silent).unwrap());
        let peer = || {
            let stream = UnixStream::connect(&socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            BufReader::new(stream)
        };
        let mut first = peer();
        let mut second = peer();
        request(&mut first, 1, Command::Subscribe);
        request(&mut second, 1, Command::Subscribe);
        first.get_mut().write_all(b"{bad json}\n").unwrap();
        let invalid: Message =
            serde_json::from_slice(&read_frame(&mut first).unwrap().unwrap()).unwrap();
        assert_eq!(invalid.error.unwrap().code, "malformed");
        request(
            &mut first,
            2,
            Command::ReplaceAndPlay {
                queue: vec![Track {
                    path: "/test.mp3".into(),
                    title: "Test".into(),
                    artist: None,
                    album: None,
                }],
                selected: 0,
            },
        );
        request(&mut second, 2, Command::SetRepeat(Repeat::All));
        request(&mut first, 3, Command::SetOrder(Order::Random));
        drop(first);
        let state = request(&mut second, 3, Command::GetState).state.unwrap();
        assert_eq!(state.current, Some(0));
        assert_eq!(state.repeat, Repeat::All);
        assert_eq!(state.order, Order::Random);
        assert_eq!(state.status, crate::protocol::Status::Playing);
        request(&mut second, 4, Command::Quit);
        server.join().unwrap();
        assert!(!socket.exists());
    }
}
