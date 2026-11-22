use crate::{
    playback::{Engine, Event, StartError, millis},
    protocol::{Command, Order, Repeat, Snapshot, Status, Track},
};
use std::time::Duration;

pub struct Controller<E> {
    engine: E,
    state: Snapshot,
    generation: u64,
    traversal: Traversal,
    pub shutdown: bool,
}

pub fn validate_queue(queue: &[Track], selected: usize) -> Result<(), &'static str> {
    if selected >= queue.len() || queue.iter().any(|track| !track.path.is_absolute()) {
        return Err("Queue needs absolute paths and a valid selection");
    }
    Ok(())
}

impl<E: Engine> Controller<E> {
    pub fn new(engine: E, repeat: Repeat, queue: Vec<Track>, restored: Option<usize>) -> Self {
        let current = restored.or_else(|| (!queue.is_empty()).then_some(0));
        let mut controller = Self {
            engine,
            state: Snapshot {
                revision: 0,
                queue: queue.into(),
                current,
                status: Status::Stopping,
                position_ms: 0,
                duration_ms: None,
                repeat,
                order: Order::Sequential,
                error: None,
            },
            generation: 0,
            traversal: Traversal::default(),
            shutdown: false,
        };
        controller.reset_cycle();
        controller
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.state.clone();
        if state.status != Status::Stopping {
            state.position_ms = millis(self.engine.position());
        }
        state
    }

    pub fn queue(&self) -> &[Track] {
        &self.state.queue
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn started_index_since(&self, generation: u64) -> Option<usize> {
        self.state
            .current
            .filter(|_| self.generation != generation && self.state.status == Status::Playing)
    }

    pub fn revision(&self) -> u64 {
        self.state.revision
    }

    pub fn command(&mut self, command: Command) -> Result<(), &'static str> {
        use Command::*;
        match command {
            Subscribe | SubscribePlayback | GetState => return Ok(()),
            UploadQueue { .. } => return Err("Unexpected queue segment"),
            ReplaceAndPlay { queue, selected } => {
                validate_queue(&queue, selected)?;
                self.state.queue = queue.into();
                self.state.current = Some(selected);
                self.reset_cycle();
                self.try_tracks(Some(selected), 1);
            }
            TogglePause => match self.state.status {
                Status::Playing => self.set_pause(true),
                Status::Pausing => self.set_pause(false),
                Status::Stopping => {}
            },
            Pause if self.state.status == Status::Playing => self.set_pause(true),
            Resume if self.state.status == Status::Pausing => self.set_pause(false),
            Pause | Resume => {}
            Stop => self.stop(),
            Restart => self.try_tracks(self.state.current, 1),
            Seek { position_ms } => self.seek(position_ms)?,
            Next | Previous => {
                let direction = if matches!(command, Next) { 1 } else { -1 };
                if let Some(index) = self.adjacent(direction) {
                    self.try_tracks(Some(index), direction);
                }
            }
            SetRepeat(repeat) => self.state.repeat = repeat,
            SetOrder(order) => {
                if self.state.order != order {
                    self.state.order = order;
                    self.reset_cycle();
                }
            }
            Quit => self.shutdown = true,
            ExitOnStopping => self.shutdown |= self.state.status == Status::Stopping,
        }
        self.state.revision += 1;
        Ok(())
    }

    fn seek(&mut self, position_ms: u64) -> Result<(), &'static str> {
        if self.state.status == Status::Stopping {
            return Err("No active track");
        }
        let position_ms = position_ms.min(self.state.duration_ms.unwrap_or(u64::MAX));
        self.engine
            .seek(Duration::from_millis(position_ms))
            .map_err(|_| "Track could not be seeked")
    }

    fn set_pause(&mut self, paused: bool) {
        self.engine.pause(paused);
        self.state.status = if paused {
            Status::Pausing
        } else {
            Status::Playing
        };
    }

    fn stop(&mut self) {
        if self.state.status != Status::Stopping {
            self.state.position_ms = millis(self.engine.position());
        }
        self.engine.stop();
        self.state.status = Status::Stopping;
    }

    fn reset_cycle(&mut self) {
        self.traversal
            .reset(self.state.queue.len(), self.state.order, self.state.current);
    }

    fn adjacent(&mut self, direction: isize) -> Option<usize> {
        self.traversal.adjacent(
            self.state.current,
            self.state.order,
            self.state.repeat,
            direction,
        )
    }

    fn try_tracks(&mut self, first: Option<usize>, direction: isize) {
        let Some(first) = first else { return };
        let mut attempted = vec![false; self.state.queue.len()];
        let mut candidate = Some(first);
        self.stop();
        while let Some(index) = candidate {
            if attempted[index] {
                break;
            }
            attempted[index] = true;
            match self.start_candidate(index) {
                Ok(()) => return,
                Err(StartError::Device(error)) => {
                    self.set_error(error);
                    return;
                }
                Err(StartError::Track(error)) => self.set_error(error),
            }
            candidate = self.adjacent(direction);
            if self.state.repeat == Repeat::All && candidate.is_some_and(|next| attempted[next]) {
                candidate = attempted.iter().position(|attempted| !attempted);
            }
        }
        self.shutdown = true;
    }

    fn start_candidate(&mut self, index: usize) -> Result<(), StartError> {
        self.state.current = Some(index);
        self.state.position_ms = 0;
        self.state.duration_ms = None;
        self.generation += 1;
        let duration = self
            .engine
            .start(&self.state.queue[index].path, self.generation)?;
        self.state.duration_ms = duration.map(millis);
        self.state.status = Status::Playing;
        self.state.error = None;
        Ok(())
    }

    pub fn set_error(&mut self, mut error: String) {
        if error.len() > 1024 {
            error.truncate(error.floor_char_boundary(1024));
        }
        self.state.error = Some(error);
    }

    pub fn tick(&mut self) {
        match self.engine.event() {
            Some(Event::End(generation))
                if generation == self.generation && self.state.status == Status::Playing =>
            {
                let next = if self.state.repeat == Repeat::One {
                    self.state.current
                } else {
                    self.adjacent(1)
                };
                if next.is_some() {
                    self.try_tracks(next, 1);
                } else {
                    self.shutdown = true;
                }
            }
            Some(Event::Device(error)) => {
                self.stop();
                self.set_error(error);
            }
            _ => return,
        }
        self.state.revision += 1;
    }
}

#[derive(Default)]
struct Traversal {
    cycle: Vec<usize>,
    rng: fastrand::Rng,
}

impl Traversal {
    fn reset(&mut self, len: usize, order: Order, current: Option<usize>) {
        self.cycle = (0..len).collect();
        if order == Order::Random
            && let Some(current) = current
        {
            self.cycle.swap(0, current);
            self.rng.shuffle(&mut self.cycle[1..]);
        }
    }

    fn adjacent(
        &mut self,
        current: Option<usize>,
        order: Order,
        repeat: Repeat,
        direction: isize,
    ) -> Option<usize> {
        let current = current?;
        let offset = self.cycle.iter().position(|&index| index == current)?;
        if let Some(index) = offset
            .checked_add_signed(direction)
            .and_then(|next| self.cycle.get(next))
        {
            return Some(*index);
        }
        if repeat != Repeat::All {
            return None;
        }
        if direction > 0 && order == Order::Random && self.cycle.len() > 1 {
            self.reset(self.cycle.len(), order, Some(current));
            self.cycle.rotate_left(1);
        }
        if direction > 0 {
            self.cycle.first().copied()
        } else {
            self.cycle.last().copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[derive(Default)]
    struct Fake {
        position: Duration,
        event: Option<Event>,
    }

    impl Engine for Fake {
        fn start(&mut self, path: &Path, _: u64) -> Result<Option<Duration>, StartError> {
            self.position = Duration::ZERO;
            if path.ends_with("broken") {
                return Err(StartError::Track("Broken".into()));
            }
            if path.ends_with("device-error") {
                return Err(StartError::Device("Unavailable".into()));
            }
            Ok(Some(Duration::from_secs(10)))
        }
        fn pause(&mut self, _: bool) {}
        fn stop(&mut self) {}
        fn seek(&mut self, position: Duration) -> Result<(), String> {
            self.position = position;
            Ok(())
        }
        fn position(&self) -> Duration {
            self.position
        }
        fn event(&mut self) -> Option<Event> {
            self.event.take()
        }
    }

    fn replace(controller: &mut Controller<Fake>, paths: &[&str]) {
        let queue = paths
            .iter()
            .map(|path| Track {
                path: path.into(),
                title: path.to_string(),
                artist: None,
                album: None,
                track_number: None,
                duration_ms: None,
                genre: None,
            })
            .collect();
        controller
            .command(Command::ReplaceAndPlay { queue, selected: 0 })
            .unwrap();
    }

    #[test]
    fn pause_stop_restart_and_exit() {
        let mut player = Controller::new(Fake::default(), Repeat::Off, Vec::new(), None);
        replace(&mut player, &["/one"]);
        player.command(Command::ExitOnStopping).unwrap();
        assert!(!player.shutdown);
        player.command(Command::Seek { position_ms: 500 }).unwrap();
        player.command(Command::Pause).unwrap();
        player.command(Command::ExitOnStopping).unwrap();
        assert!(!player.shutdown);
        player.command(Command::Resume).unwrap();
        assert_eq!(player.snapshot().position_ms, 500);
        player.command(Command::Stop).unwrap();
        player.command(Command::Restart).unwrap();
        assert_eq!(player.snapshot().position_ms, 0);
        player.command(Command::Stop).unwrap();
        player.command(Command::ExitOnStopping).unwrap();
        assert!(player.shutdown);
    }

    #[test]
    fn skips_broken_tracks_and_ignores_old_end() {
        let mut player = Controller::new(Fake::default(), Repeat::Off, Vec::new(), None);
        replace(&mut player, &["/one"]);
        let old = player.generation;
        player.command(Command::Pause).unwrap();
        replace(&mut player, &["/broken", "/two"]);
        assert_eq!(player.snapshot().current, Some(1));
        assert_eq!(player.generation, old + 2);
        player.engine.event = Some(Event::End(old + 1));
        player.tick();
        assert!(!player.shutdown);
        player.engine.event = Some(Event::End(player.generation));
        player.tick();
        assert!(player.shutdown);
    }

    #[test]
    fn random_previous_retraces_order_and_repeat_one_only_affects_eof() {
        let mut player = Controller::new(Fake::default(), Repeat::Off, Vec::new(), None);
        player.traversal.rng = fastrand::Rng::with_seed(123);
        player.command(Command::SetOrder(Order::Random)).unwrap();
        replace(&mut player, &["/one", "/two", "/three"]);
        player.command(Command::SetRepeat(Repeat::One)).unwrap();
        player.command(Command::Next).unwrap();
        let second = player.snapshot().current;
        assert_ne!(second, Some(0));
        let generation = player.generation;
        player.engine.event = Some(Event::End(generation));
        player.tick();
        assert_eq!(player.snapshot().current, second);
        assert_eq!(player.generation, generation + 1);
        player.command(Command::Previous).unwrap();
        assert_eq!(player.snapshot().current, Some(0));
        player.command(Command::SetRepeat(Repeat::Off)).unwrap();
        player.command(Command::Next).unwrap();
        assert_eq!(player.snapshot().current, second);
        player.command(Command::Next).unwrap();
        assert_ne!(player.snapshot().current, second);
        player.engine.event = Some(Event::End(player.generation));
        player.tick();
        assert!(player.shutdown);
    }

    #[test]
    fn device_failure_ends_search_without_shutdown() {
        let mut player = Controller::new(Fake::default(), Repeat::All, Vec::new(), None);
        replace(&mut player, &["/device-error", "/two"]);
        let snapshot = player.snapshot();
        assert_eq!(snapshot.status, Status::Stopping);
        assert_eq!(snapshot.current, Some(0));
        assert_eq!(snapshot.error.as_deref(), Some("Unavailable"));
        assert_eq!(player.generation, 1);
        assert!(!player.shutdown);
    }

    #[test]
    fn all_broken_tracks_exhaust_even_with_repeat_all() {
        let mut player = Controller::new(Fake::default(), Repeat::Off, Vec::new(), None);
        player.command(Command::SetRepeat(Repeat::All)).unwrap();
        player.command(Command::SetOrder(Order::Random)).unwrap();
        replace(
            &mut player,
            &["/one/broken", "/two/broken", "/three/broken"],
        );
        assert!(player.shutdown);
        assert_eq!(player.generation, 3);
    }
}
