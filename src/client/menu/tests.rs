use super::*;

fn summary(status: Status, has_queue: bool) -> PlaybackSummary {
    PlaybackSummary {
        status,
        title: has_queue.then(|| "Song".into()),
        artist: None,
        duration_ms: None,
        repeat: Repeat::Off,
        order: Order::Sequential,
        has_queue,
        error: None,
    }
}

#[test]
fn maps_playback_actions_to_authoritative_state() {
    for (status, action, expected) in [
        (Status::Stopping, Action::Play, "Restart"),
        (Status::Pausing, Action::Play, "Resume"),
        (Status::Playing, Action::Pause, "Pause"),
    ] {
        let state = State {
            summary: Some(summary(status, true)),
            ..State::default()
        };
        assert_eq!(state.playback_action(), action);
        assert_eq!(format!("{:?}", state.command(action).unwrap()), expected);
        assert!(matches!(state.command(Action::Next), Some(Command::Next)));
        assert!(matches!(
            state.command(Action::Previous),
            Some(Command::Previous)
        ));
        assert!(matches!(
            state.command(Action::Repeat(Repeat::One)),
            Some(Command::SetRepeat(Repeat::One))
        ));
        assert!(matches!(
            state.command(Action::Order(Order::Random)),
            Some(Command::SetOrder(Order::Random))
        ));
    }
    let playing = State {
        summary: Some(summary(Status::Playing, true)),
        ..State::default()
    };
    assert!(playing.command(Action::Play).is_none());
}

#[test]
fn offline_empty_and_busy_states_gate_controls() {
    let mut state = State::default();
    assert_eq!(state.info_text(), "Server offline");
    assert!(state.command(Action::Repeat(Repeat::All)).is_none());
    assert!(matches!(state.command(Action::Exit), Some(Command::Quit)));

    state.summary = Some(summary(Status::Stopping, false));
    assert_eq!(state.info_text(), "Queue empty — add songs in the TUI");
    for action in [Action::Play, Action::Next, Action::Previous] {
        assert!(state.command(action).is_none());
    }

    state.summary = Some(summary(Status::Stopping, true));
    state.busy = Some("Starting server");
    assert!(state.command(Action::Play).is_none());
}

#[test]
fn info_row_shows_track_and_replaces_it_with_notices() {
    let mut summary = summary(Status::Playing, true);
    summary.artist = Some("A & B".into());
    summary.duration_ms = Some(185_999);
    let mut state = State {
        summary: Some(summary),
        ..State::default()
    };
    for status in [Status::Playing, Status::Pausing, Status::Stopping] {
        state.summary.as_mut().unwrap().status = status;
        assert_eq!(state.info_text(), "A & B (3:05) Song");
    }

    state.summary.as_mut().unwrap().artist = None;
    assert_eq!(state.info_text(), "(3:05) Song");
    state.summary.as_mut().unwrap().duration_ms = None;
    assert_eq!(state.info_text(), "Song");

    state.summary.as_mut().unwrap().error = Some("Playback failed".into());
    assert_eq!(state.info_text(), "Playback failed");
    state.busy = Some("Stopping server");
    assert_eq!(state.info_text(), "Stopping server");
}
