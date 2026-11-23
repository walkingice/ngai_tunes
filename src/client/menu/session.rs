use super::{Action, State};
use crate::{
    client::ipc::{self, Transport},
    protocol::{Command, Message, PlaybackSummary, WireMessage},
};
use crossbeam_channel::{self as channel, Receiver};
use std::{
    io, thread,
    time::{Duration, Instant},
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

enum Event {
    Summary(Option<u64>, PlaybackSummary),
    Reply(Message),
}

type Connection = Transport<Event>;

pub(super) enum Update {
    State(State),
    Close,
}

struct Session {
    state: State,
    connection: Option<Connection>,
    retry: Instant,
    delay: Duration,
    pending: Option<(u64, Instant)>,
}

pub(super) fn spawn(actions: Receiver<Action>, publish: impl Fn(Update) + Send + 'static) {
    thread::spawn(move || {
        let mut session = Session::new();
        session.reconnect(connect);
        loop {
            publish(Update::State(session.state.clone()));
            if !session.step(&actions, &publish) {
                return;
            }
        }
    });
}

fn connect() -> io::Result<(Connection, PlaybackSummary)> {
    let mut connection = Transport::open(decode)?;
    let subscription = connection.send(Command::SubscribePlayback)?;
    let failure = match connection.receiver.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(Event::Summary(Some(id), summary))) if id == subscription => {
            return Ok((connection, summary));
        }
        Ok(Ok(Event::Reply(reply))) => reply.error.map(|error| error.message),
        Ok(Err(error)) => Some(error.to_string()),
        _ => None,
    };
    Err(io::Error::other(format!(
        "Server does not support playback summaries. Restart it with this version.{}",
        failure.map_or_else(String::new, |error| format!(" ({error})"))
    )))
}

fn decode(wire: WireMessage) -> io::Result<Option<Event>> {
    match wire {
        WireMessage::Playback {
            request_id,
            summary,
        } => Ok(Some(Event::Summary(request_id, summary))),
        WireMessage::Message { message, .. } if message.state.is_none() => {
            Ok(Some(Event::Reply(message)))
        }
        _ => Err(io::Error::other(
            "Unexpected response to playback subscription",
        )),
    }
}

impl Session {
    fn new() -> Self {
        Self {
            state: State::default(),
            connection: None,
            retry: Instant::now(),
            delay: Duration::from_secs(1),
            pending: None,
        }
    }

    fn reconnect(&mut self, connect: impl FnOnce() -> io::Result<(Connection, PlaybackSummary)>) {
        match connect() {
            Ok((connection, summary)) => {
                self.connection = Some(connection);
                self.state.summary = Some(summary);
                self.state.error = None;
                self.state.connection_error = None;
                self.delay = Duration::from_secs(1);
            }
            Err(error) => {
                self.state.connection_error =
                    (!ipc::is_offline(&error)).then(|| format!("Could not connect: {error}"));
                self.defer();
            }
        }
    }

    fn defer(&mut self) {
        self.connection = None;
        self.state.summary = None;
        self.state.busy = None;
        self.pending = None;
        self.retry = Instant::now() + self.delay;
        self.delay = (self.delay * 2).min(Duration::from_secs(30));
    }

    fn step(&mut self, actions: &Receiver<Action>, publish: &impl Fn(Update)) -> bool {
        let never = channel::never();
        let (incoming, timeout) = match &self.connection {
            Some(connection) => (
                connection.receiver.clone(),
                self.pending.map(|(_, time)| time),
            ),
            None => (never, Some(self.retry)),
        };
        let timer = timeout.map_or_else(channel::never, channel::at);
        channel::select_biased! {
            recv(incoming) -> event => self.receive(event),
            recv(actions) -> action => match action {
                Ok(action) => return self.act(action, actions, publish),
                Err(_) => return false,
            },
            recv(timer) -> _ => {
                if self.connection.is_none() {
                    self.reconnect(connect);
                } else {
                    self.pending = None;
                    self.state.busy = None;
                    self.state.error = Some("Server did not acknowledge the command".into());
                }
            }
        }
        true
    }

    fn receive(&mut self, event: Result<io::Result<Event>, channel::RecvError>) {
        match event {
            Ok(Ok(Event::Summary(None, summary))) => self.state.summary = Some(summary),
            Ok(Ok(Event::Reply(reply)))
                if self
                    .pending
                    .is_some_and(|(id, _)| reply.request_id == Some(id)) =>
            {
                self.pending = None;
                self.state.busy = None;
                self.state.error = reply.error.map(|error| error.message);
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                self.state.error = Some(format!("Server connection failed: {error}"));
                self.defer();
            }
            Err(_) => {
                self.state.error = self
                    .pending
                    .is_some()
                    .then(|| "Server disconnected before acknowledging the command".into());
                self.defer();
            }
        }
    }

    fn act(
        &mut self,
        action: Action,
        actions: &Receiver<Action>,
        publish: &impl Fn(Update),
    ) -> bool {
        if action == Action::Exit {
            self.state.busy = Some("Stopping server");
            publish(Update::State(self.state.clone()));
            match self.quit() {
                Ok(()) => {
                    publish(Update::Close);
                    return false;
                }
                Err(error) => {
                    self.state.busy = None;
                    self.state.error = Some(format!("Could not stop server: {error}"));
                }
            }
            return true;
        }
        if action == Action::Play && self.connection.is_none() {
            return self.play_offline(actions, publish, || ipc::start(connect));
        }
        self.send(action);
        true
    }

    fn play_offline(
        &mut self,
        actions: &Receiver<Action>,
        publish: &impl Fn(Update),
        start: impl FnOnce() -> io::Result<(Connection, PlaybackSummary)>,
    ) -> bool {
        self.state.busy = Some("Starting server");
        self.state.error = None;
        publish(Update::State(self.state.clone()));
        self.reconnect(start);
        self.state.busy = None;
        // Clicks received while Play was starting belong to that attempt.
        while let Ok(queued) = actions.try_recv() {
            if queued == Action::Exit {
                return self.act(queued, actions, publish);
            }
        }
        self.send(Action::Play);
        true
    }

    fn send(&mut self, action: Action) {
        if let Some(command) = self.state.command(action)
            && let Some(connection) = &mut self.connection
        {
            match connection.send(command) {
                Ok(id) => {
                    self.pending = Some((id, Instant::now() + REPLY_TIMEOUT));
                    self.state.busy = Some("Waiting for server");
                    self.state.error = None;
                }
                Err(error) => {
                    self.state.error = Some(format!("Could not send command: {error}"));
                    self.defer();
                }
            }
        }
    }

    fn quit(&mut self) -> io::Result<()> {
        self.quit_with(|| Transport::open(decode), ipc::open_socket)
    }

    fn quit_with<T>(
        &mut self,
        open: impl FnOnce() -> io::Result<Connection>,
        probe: impl Fn() -> io::Result<T>,
    ) -> io::Result<()> {
        let mut temporary;
        let connection = match self.connection.as_mut() {
            Some(connection) => connection,
            None => match open() {
                Ok(connection) => {
                    temporary = connection;
                    &mut temporary
                }
                Err(error) if ipc::is_offline(&error) => return Ok(()),
                Err(error) => return Err(error),
            },
        };
        let deadline = Instant::now() + REPLY_TIMEOUT;
        if let Ok(id) = connection.send(Command::Quit) {
            while let Ok(Ok(event)) = connection.receiver.recv_deadline(deadline) {
                match event {
                    Event::Reply(reply) if reply.request_id == Some(id) => {
                        if let Some(error) = reply.error {
                            return Err(io::Error::other(error.message));
                        }
                        break;
                    }
                    Event::Summary(_, summary) => self.state.summary = Some(summary),
                    _ => {}
                }
            }
        }
        loop {
            match probe() {
                Err(error) if ipc::is_offline(&error) => return Ok(()),
                Err(error) => return Err(error),
                Ok(_) if Instant::now() >= deadline => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Server is still reachable",
                    ));
                }
                Ok(_) => thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Failure, Order, Repeat, Request, Status, read_frame};
    use std::{io::BufReader, os::unix::net::UnixStream};

    fn pair() -> (Connection, BufReader<UnixStream>) {
        let (client, server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        (
            Transport::from_stream(client, decode).unwrap(),
            BufReader::new(server),
        )
    }

    fn request(server: &mut BufReader<UnixStream>) -> Request {
        let frame = read_frame(server).unwrap().unwrap();
        serde_json::from_slice(&frame).unwrap()
    }

    #[test]
    fn offline_play_coalesces_clicks_and_uses_restored_state() {
        for (status, has_queue) in [
            (Status::Stopping, true),
            (Status::Stopping, false),
            (Status::Playing, true),
        ] {
            let mut session = Session::new();
            session.reconnect(|| Err(io::ErrorKind::NotFound.into()));
            let (send, actions) = channel::unbounded();
            let (connection, mut server) = pair();
            let summary = PlaybackSummary {
                status,
                title: has_queue.then(|| "Song".into()),
                artist: None,
                duration_ms: None,
                repeat: Repeat::Off,
                order: Order::Sequential,
                has_queue,
                error: None,
            };
            assert!(session.play_offline(&actions, &|update| {
                assert!(matches!(update, Update::State(state) if state.busy == Some("Starting server")));
                send.send(Action::Play).unwrap();
                send.send(Action::Play).unwrap();
            }, || Ok((connection, summary))));
            assert!(actions.is_empty());
            if status == Status::Stopping && has_queue {
                assert!(matches!(request(&mut server).command, Command::Restart));
            } else {
                assert!(session.pending.is_none());
            }
        }
    }

    #[test]
    fn command_failures_and_disconnect_clear_pending_controls() {
        let (connection, server) = pair();
        let mut session = Session::new();
        session.connection = Some(connection);
        session.pending = Some((3, Instant::now()));
        session.state.busy = Some("Waiting for server");
        session.receive(Ok(Ok(Event::Reply(Message {
            request_id: Some(2),
            state: None,
            error: None,
        }))));
        assert!(session.state.busy.is_some());
        session.receive(Ok(Ok(Event::Reply(Message {
            request_id: Some(3),
            state: None,
            error: Some(Failure {
                code: "invalid_command".into(),
                message: "Cannot play track".into(),
            }),
        }))));
        assert_eq!(session.state.error.as_deref(), Some("Cannot play track"));
        assert!(session.pending.is_none());
        session.state.summary = Some(PlaybackSummary {
            status: Status::Playing,
            title: Some("Song".into()),
            artist: None,
            duration_ms: None,
            repeat: Repeat::Off,
            order: Order::Sequential,
            has_queue: true,
            error: None,
        });
        drop(server);
        let event = session
            .connection
            .as_ref()
            .unwrap()
            .receiver
            .recv_timeout(REPLY_TIMEOUT);
        assert!(matches!(
            event,
            Err(channel::RecvTimeoutError::Disconnected)
        ));
        session.receive(Err(channel::RecvError));
        assert_eq!(session.state.info_text(), "Server offline");
        assert!(session.connection.is_none());
    }

    #[test]
    fn offline_exit_probes_during_backoff_and_accepts_a_lost_reply() {
        let mut session = Session::new();
        session.defer();
        assert!(session.retry > Instant::now());
        let (connection, mut server) = pair();
        let daemon = thread::spawn(move || {
            let request = request(&mut server);
            assert!(matches!(request.command, Command::Quit));
        });
        session
            .quit_with(
                || Ok(connection),
                || Err::<(), _>(io::ErrorKind::NotFound.into()),
            )
            .unwrap();
        daemon.join().unwrap();
    }
}
