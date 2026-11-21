mod controller;
pub mod runtime;

use crate::{
    playback::{Audio, Engine},
    protocol::{
        Command, Failure, Message, QueueSegment, QueueTransfer, Request, Snapshot, Track, VERSION,
        WireMessage, encode, encode_segments, read_frame,
    },
    storage::config::ConfigStore,
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
    queue: Arc<[Frame]>,
    close: bool,
    delivered: Option<mpsc::Sender<()>>,
}

impl Output {
    fn normal(frame: Frame) -> Self {
        Self {
            frame,
            queue: Arc::default(),
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
    queue_revision: Option<u64>,
    upload: QueueTransfer,
    selection: Option<usize>,
    graceful: bool,
}

impl Drop for Peer {
    fn drop(&mut self) {
        if !self.graceful {
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

struct QueueFrames {
    revision: u64,
    frames: Arc<[Frame]>,
}

impl QueueFrames {
    fn new(revision: u64, tracks: &[Track]) -> io::Result<Self> {
        let frames = encode_segments(tracks, |segment, _| {
            encode(&WireMessage::Queue { revision, segment })
        })?;
        Ok(Self {
            revision,
            frames: frames.into_iter().map(Frame::from).collect(),
        })
    }

    fn output(&self, peer: &mut Peer, mut output: Output) -> Result<(), ()> {
        if peer.queue_revision != Some(self.revision) {
            output.queue = self.frames.clone();
        }
        peer.output.try_send(output).map_err(|_| ())?;
        peer.queue_revision = Some(self.revision);
        Ok(())
    }
}

impl Peer {
    fn upload(
        &mut self,
        id: u64,
        segment: QueueSegment,
        selected: Option<usize>,
    ) -> Result<Option<(Vec<Track>, usize)>, &'static str> {
        if (segment.segment_index == 1) != selected.is_some() {
            self.upload = QueueTransfer::default();
            self.selection = None;
            return Err("Queue selection must be on the first segment");
        }
        if let Some(selected) = selected {
            self.selection = Some(selected);
        }
        let Some(queue) = self.upload.push(id, segment)? else {
            return Ok(None);
        };
        let selected = self.selection.take().ok_or("Missing queue selection")?;
        Ok(Some((queue, selected)))
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = ConfigStore::user().ok();
    serve(
        Runtime::bind(Path::new(runtime::ROOT))?,
        Audio::default(),
        config.as_ref(),
    )?;
    Ok(())
}

fn serve(runtime: Runtime, engine: impl Engine, config: Option<&ConfigStore>) -> io::Result<()> {
    let (input, requests) = mpsc::sync_channel(64);
    let mut peers = HashMap::new();
    let mut id = 0;
    let settings = config
        .and_then(|store| store.load().ok())
        .unwrap_or_default();
    let mut controller = Controller::new(engine, settings.repeat);
    let mut queue = QueueFrames::new(0, &[])?;
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
            Ok(Input::Request(id, request)) => {
                respond(&mut controller, &mut queue, &mut peers, config, id, request)
            }
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
            let state = WireMessage::Message {
                queue_revision: Some(queue.revision),
                message: message(None, Some(controller.snapshot()), None),
            };
            if let Ok(frame) = encode(&state) {
                let frame: Frame = frame.into();
                peers.retain(|_, peer| {
                    !peer.subscribed || queue.output(peer, Output::normal(frame.clone())).is_ok()
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
    queue: &mut QueueFrames,
    peers: &mut HashMap<u64, Peer>,
    config: Option<&ConfigStore>,
    id: u64,
    request: Request,
) {
    let Some(peer) = peers.get_mut(&id) else {
        return;
    };
    let Some(response) = handle_request(controller, queue, peer, config, request) else {
        return;
    };
    let has_state = response.state.is_some();
    let response = WireMessage::Message {
        queue_revision: has_state.then_some(queue.revision),
        message: response,
    };
    let (delivered, wait) = mpsc::channel();
    let closing = controller.shutdown;
    let sent = encode(&response).is_ok_and(|frame| {
        let output = Output {
            frame: frame.into(),
            queue: Arc::default(),
            close: closing,
            delivered: closing.then_some(delivered),
        };
        if has_state {
            queue.output(peer, output).is_ok()
        } else {
            peer.output.try_send(output).is_ok()
        }
    });
    if !sent {
        peers.remove(&id);
    } else if closing {
        peer.graceful = wait.recv_timeout(Duration::from_millis(100)).is_ok();
    }
}

fn handle_request(
    controller: &mut Controller<impl Engine>,
    queue: &mut QueueFrames,
    peer: &mut Peer,
    config: Option<&ConfigStore>,
    request: Request,
) -> Option<Message> {
    let id = Some(request.request_id);
    let failure = |code, description| Some(message(id, None, Some((code, description))));
    if request.version != VERSION {
        return failure("version", "Unsupported protocol version");
    }

    if let Command::UploadQueue { segment, selected } = request.command {
        return complete_upload(
            controller,
            queue,
            peer,
            request.request_id,
            segment,
            selected,
        );
    }

    let subscribe = matches!(request.command, Command::Subscribe);
    let snapshot = subscribe || matches!(request.command, Command::GetState);
    if let Command::SetRepeat(repeat) = request.command {
        let Some(config) = config else {
            return failure("config", "Could not save repeat mode: HOME is not set");
        };
        if let Err(error) = config.save_repeat(repeat) {
            return failure("config", &format!("Could not save repeat mode: {error}"));
        }
    }
    match controller.command(request.command) {
        Ok(()) => {
            peer.subscribed |= subscribe;
            Some(message(id, snapshot.then(|| controller.snapshot()), None))
        }
        Err(error) => failure("invalid_command", error),
    }
}

fn complete_upload(
    controller: &mut Controller<impl Engine>,
    queue: &mut QueueFrames,
    peer: &mut Peer,
    request_id: u64,
    segment: QueueSegment,
    selected: Option<usize>,
) -> Option<Message> {
    let id = Some(request_id);
    let failure = |code, description| Some(message(id, None, Some((code, description))));
    let (tracks, selected) = match peer.upload(request_id, segment, selected) {
        // Acknowledge only when the full upload can be accepted or rejected.
        Ok(pending) => pending?,
        Err(error) => return failure("invalid_segment", error),
    };
    let Ok(frames) = QueueFrames::new(controller.revision() + 1, &tracks) else {
        return failure("frame_size", "A track is too large for a queue segment");
    };
    match controller.command(Command::ReplaceAndPlay {
        queue: tracks,
        selected,
    }) {
        Ok(()) => {
            *queue = frames;
            Some(message(id, None, None))
        }
        Err(error) => failure("invalid_command", error),
    }
}

fn connect_peer(id: u64, stream: UnixStream, input: SyncSender<Input>) -> io::Result<Peer> {
    stream.set_nonblocking(false)?;
    let mut writer = stream.try_clone()?;
    writer.set_write_timeout(Some(Duration::from_secs(1)))?;
    let reader = stream.try_clone()?;
    let (output, frames) = mpsc::sync_channel::<Output>(8);
    let errors = output.clone();
    let writer_input = input.clone();
    thread::spawn(move || {
        for output in frames {
            if output
                .queue
                .iter()
                .chain(std::iter::once(&output.frame))
                .try_for_each(|frame| writer.write_all(frame))
                .is_err()
            {
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
        queue_revision: None,
        upload: QueueTransfer::default(),
        selection: None,
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
        if let Ok(frame) = encode(&WireMessage::reply(message(
            None,
            None,
            Some((code, description)),
        ))) && errors
            .try_send(Output {
                frame: frame.into(),
                queue: Arc::default(),
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
        protocol::{IncomingState, Order, Repeat, Track, encode_upload},
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

    struct TestPeer {
        stream: BufReader<UnixStream>,
        incoming: IncomingState,
    }

    impl TestPeer {
        fn write(&mut self, frame: &[u8]) {
            for chunk in frame.chunks(7) {
                self.stream.get_mut().write_all(chunk).unwrap();
            }
        }

        fn read(&mut self) -> Message {
            loop {
                let frame = read_frame(&mut self.stream)
                    .unwrap()
                    .expect("Unexpected EOF");
                let wire = serde_json::from_slice(&frame).unwrap();
                if let Some(message) = self.incoming.receive(wire).unwrap() {
                    return message;
                }
            }
        }

        fn reply(&mut self, id: u64) -> Message {
            loop {
                let message = self.read();
                if message.request_id == Some(id) {
                    return message;
                }
            }
        }

        fn request(&mut self, id: u64, command: Command) -> Message {
            let frame = encode(&Request {
                version: VERSION,
                request_id: id,
                command,
            })
            .unwrap();
            self.write(&frame);
            let message = self.reply(id);
            assert!(message.error.is_none());
            message
        }
    }

    fn test_peer(socket: &Path) -> TestPeer {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        TestPeer {
            stream: BufReader::new(stream),
            incoming: IncomingState::default(),
        }
    }

    fn track(title: String) -> Track {
        Track {
            path: "/test.mp3".into(),
            title,
            artist: None,
            album: None,
            track_number: None,
            duration_ms: None,
            genre: None,
        }
    }

    #[test]
    fn two_clients_share_state_and_quit_is_acknowledged() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("run");
        let runtime = Runtime::bind(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let config = ConfigStore::new(root.path().join("config.json"));
        let server = thread::spawn(move || serve(runtime, Silent, Some(&config)).unwrap());
        let peer = || test_peer(&socket);
        let mut first = peer();
        let mut second = peer();
        first.request(1, Command::Subscribe);
        second.request(1, Command::Subscribe);
        first.write(b"{bad json}\n");
        assert_eq!(first.read().error.unwrap().code, "malformed");
        let track = track("Test".into());
        for frame in encode_upload(2, &[track], 0).unwrap() {
            first.write(&frame);
        }
        assert!(first.reply(2).error.is_none());
        second.request(2, Command::SetRepeat(Repeat::All));
        first.request(3, Command::SetOrder(Order::Random));
        drop(first);
        let state = second.request(3, Command::GetState).state.unwrap();
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.current, Some(0));
        assert_eq!(state.repeat, Repeat::All);
        assert_eq!(state.order, Order::Random);
        assert_eq!(state.status, crate::protocol::Status::Playing);
        second.request(4, Command::Quit);
        server.join().unwrap();
        assert!(!socket.exists());
    }
    #[test]
    fn repeat_mode_survives_daemon_restart() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("run");
        let socket = directory.join("daemon.sock");
        let start = || {
            let runtime = Runtime::bind(&directory).unwrap();
            let config = ConfigStore::new(root.path().join("config.json"));
            thread::spawn(move || serve(runtime, Silent, Some(&config)).unwrap())
        };
        let server = start();
        let mut peer = test_peer(&socket);
        peer.request(1, Command::SetRepeat(Repeat::One));
        peer.request(2, Command::Quit);
        server.join().unwrap();

        let server = start();
        let mut peer = test_peer(&socket);
        let state = peer.request(1, Command::Subscribe).state.unwrap();
        assert_eq!(state.repeat, Repeat::One);
        peer.request(2, Command::Quit);
        server.join().unwrap();
    }

    #[test]
    fn large_queue_is_atomic_and_shared_with_new_subscribers() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("run");
        let runtime = Runtime::bind(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let config = ConfigStore::new(root.path().join("config.json"));
        let server = thread::spawn(move || serve(runtime, Silent, Some(&config)).unwrap());
        let mut observer = test_peer(&socket);
        observer.request(1, Command::Subscribe);
        let mut uploader = test_peer(&socket);
        let initial = encode_upload(1, &[track("Initial".into())], 0).unwrap();
        uploader.write(&initial[0]);
        assert!(uploader.reply(1).error.is_none());

        let queue: Vec<_> = (0..205)
            .map(|index| track(format!("{index} {}", "x".repeat(6000))))
            .collect();
        assert!(serde_json::to_vec(&queue).unwrap().len() > crate::protocol::MAX_FRAME);
        let frames = encode_upload(2, &queue, 204).unwrap();
        assert_eq!(frames.len(), 3);
        uploader.stream.get_mut().write_all(&frames[0]).unwrap();
        let previous = uploader.request(3, Command::GetState).state.unwrap();
        assert_eq!(previous.queue[0].title, "Initial");
        let previous = observer.request(2, Command::GetState).state.unwrap();
        assert_eq!(previous.queue[0].title, "Initial");

        uploader.stream.get_mut().write_all(&frames[2]).unwrap();
        assert_eq!(uploader.reply(2).error.unwrap().code, "invalid_segment");
        drop(uploader);
        let previous = observer.request(3, Command::GetState).state.unwrap();
        assert_eq!(previous.queue[0].title, "Initial");

        let mut uploader = test_peer(&socket);
        let invalid = encode_upload(1, &[track("Rejected".into())], 1).unwrap();
        uploader.write(&invalid[0]);
        assert_eq!(uploader.reply(1).error.unwrap().code, "invalid_command");
        let previous = observer.request(4, Command::GetState).state.unwrap();
        assert_eq!(previous.queue[0].title, "Initial");
        assert_eq!(previous.current, Some(0));

        for frame in frames {
            uploader.stream.get_mut().write_all(&frame).unwrap();
        }
        assert!(uploader.reply(2).error.is_none());
        let broadcast = observer.read().state.unwrap();
        assert_eq!(*broadcast.queue, queue);
        assert_eq!(broadcast.current, Some(204));

        let mut attached = test_peer(&socket);
        let snapshot = attached.request(1, Command::Subscribe).state.unwrap();
        assert_eq!(*snapshot.queue, queue);
        assert_eq!(snapshot.current, Some(204));
        attached.request(2, Command::SetRepeat(Repeat::One));
        let updated = attached.read().state.unwrap();
        assert_eq!(updated.repeat, Repeat::One);
        assert!(Arc::ptr_eq(&snapshot.queue, &updated.queue));
        attached.request(3, Command::Quit);
        server.join().unwrap();
    }
}
