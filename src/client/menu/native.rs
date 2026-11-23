use super::{
    Action, Order, Repeat, State,
    marquee::{INTERVAL, Marquee},
    session::{self, Update},
};
use crossbeam_channel::{self as channel, Sender};
use std::{cell::RefCell, error::Error, rc::Rc};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, Submenu},
};

const REPEAT: [Repeat; 3] = [Repeat::Off, Repeat::All, Repeat::One];
const ORDER: [Order; 2] = [Order::Sequential, Order::Random];

struct InfoRow {
    item: MenuItem,
    marquee: RefCell<Marquee>,
}

impl InfoRow {
    fn set_text(&self, text: &str) {
        self.item.set_text(text.replace('&', "&&"));
    }

    fn advance(&self) {
        if let Some(text) = self.marquee.borrow_mut().advance() {
            self.set_text(&text);
        }
    }
}

struct Tray {
    _icon: TrayIcon,
    menu: Menu,
    info: Rc<InfoRow>,
    play: MenuItem,
    pause: MenuItem,
    next: MenuItem,
    previous: MenuItem,
    exit: MenuItem,
    repeat: Submenu,
    order: Submenu,
    repeat_items: [CheckMenuItem; 3],
    order_items: [CheckMenuItem; 2],
    state: State,
}

impl Tray {
    fn new() -> Result<Self, Box<dyn Error>> {
        let menu = Menu::new();
        let repeat = Submenu::new("↻ Repeat", false);
        let order = Submenu::new("→ Order", false);
        let repeat_items = ["Off", "All", "One"].map(check_item);
        let order_items = ["Sequential", "Random"].map(check_item);
        for item in &repeat_items {
            repeat.append(item)?;
        }
        for item in &order_items {
            order.append(item)?;
        }
        let status = MenuItem::new("Connecting to server", false, None);
        let play = MenuItem::new("▶ Play", false, None);
        let next = MenuItem::new("≫ Next", false, None);
        let previous = MenuItem::new("≪ Previous", false, None);
        let exit = MenuItem::new("Exit", true, None);
        menu.append_items(&[&status, &play, &next, &previous, &repeat, &order, &exit])?;
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("Ngai Tunes");
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(make_icon()?);
        #[cfg(target_os = "linux")]
        let builder = builder.with_icon(make_icon()?);
        let icon = builder.build()?;
        Ok(Self {
            _icon: icon,
            menu,
            info: Rc::new(InfoRow {
                item: status,
                marquee: RefCell::default(),
            }),
            play,
            pause: MenuItem::new("⏸ Pause", false, None),
            next,
            previous,
            exit,
            repeat,
            order,
            repeat_items,
            order_items,
            state: State::default(),
        })
    }

    fn apply(&mut self, state: State) {
        let summary = state.summary.as_ref();
        if let Some(text) = self.info.marquee.borrow_mut().set(state.info_text()) {
            self.info.set_text(&text);
        }

        let action = state.playback_action();
        let (active, hidden, label) = match action {
            Action::Pause => (&self.pause, &self.play, "⏸ Pause"),
            _ => (&self.play, &self.pause, "▶ Play"),
        };
        self.row(hidden, None, 1);
        self.row(active, Some(label), 1);
        active.set_enabled(
            state.busy.is_none() && (summary.is_none() || state.command(action).is_some()),
        );
        self.next.set_enabled(state.command(Action::Next).is_some());
        self.previous
            .set_enabled(state.command(Action::Previous).is_some());
        self.repeat
            .set_enabled(summary.is_some() && state.busy.is_none());
        self.order
            .set_enabled(summary.is_some() && state.busy.is_none());
        self.order
            .set_text(match summary.map(|summary| summary.order) {
                Some(Order::Random) => "⤨ Order",
                _ => "→ Order",
            });
        self.exit.set_enabled(state.busy != Some("Stopping server"));
        self.state = state;
        self.apply_checks();
    }

    fn row(&self, item: &MenuItem, text: Option<&str>, position: usize) -> bool {
        let present = self
            .menu
            .items()
            .iter()
            .any(|child| child.id() == item.id());
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            let text = text.replace('&', "&&");
            if item.text() != text {
                item.set_text(text);
            }
            return present || self.menu.insert(item, position).is_ok();
        }
        if present {
            let _ = self.menu.remove(item);
        }
        false
    }

    fn apply_checks(&self) {
        for (item, repeat) in self.repeat_items.iter().zip(REPEAT) {
            item.set_checked(
                self.state
                    .summary
                    .as_ref()
                    .is_some_and(|s| s.repeat == repeat),
            );
        }
        for (item, order) in self.order_items.iter().zip(ORDER) {
            item.set_checked(
                self.state
                    .summary
                    .as_ref()
                    .is_some_and(|s| s.order == order),
            );
        }
    }

    fn click(&self, event: MenuEvent, actions: &Sender<Action>) {
        let action = if event.id == *self.play.id() {
            Some(Action::Play)
        } else if event.id == *self.pause.id() {
            Some(Action::Pause)
        } else if event.id == *self.next.id() {
            Some(Action::Next)
        } else if event.id == *self.previous.id() {
            Some(Action::Previous)
        } else if event.id == *self.exit.id() {
            Some(Action::Exit)
        } else {
            self.repeat_items
                .iter()
                .zip(REPEAT)
                .find_map(|(item, repeat)| {
                    (event.id == *item.id()).then_some(Action::Repeat(repeat))
                })
                .or_else(|| {
                    self.order_items
                        .iter()
                        .zip(ORDER)
                        .find_map(|(item, order)| {
                            (event.id == *item.id()).then_some(Action::Order(order))
                        })
                })
        };
        self.apply_checks();
        if let Some(action) = action {
            let _ = actions.send(action);
        }
    }
}

fn check_item(label: &str) -> CheckMenuItem {
    CheckMenuItem::new(label, true, false, None)
}

fn make_icon() -> Result<Icon, Box<dyn Error>> {
    let svg = include_bytes!("icon.svg");
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(64, 64).ok_or("Could not allocate icon")?;
    let scale = resvg::tiny_skia::Transform::from_scale(
        64.0 / tree.size().width(),
        64.0 / tree.size().height(),
    );
    resvg::render(&tree, scale, &mut pixmap.as_mut());
    // The SVG is a monochrome mask. Set unpremultiplied RGB for tray-icon.
    for pixel in pixmap.data_mut().chunks_exact_mut(4) {
        #[cfg(target_os = "linux")]
        pixel[..3].copy_from_slice(&[66, 153, 225]);
        #[cfg(target_os = "macos")]
        pixel[..3].fill(0);
    }
    Ok(Icon::from_rgba(pixmap.take(), 64, 64)?)
}

#[cfg(target_os = "linux")]
pub(super) fn run() -> Result<(), Box<dyn Error>> {
    let mut tray = Tray::new()?;
    let (actions, requests) = channel::unbounded();
    let (publish, updates) = channel::unbounded();
    session::spawn(requests, move |update| {
        let _ = publish.send(update);
    });
    let ticks = channel::tick(INTERVAL);
    loop {
        channel::select! {
            recv(ticks) -> _ => tray.info.advance(),
            recv(MenuEvent::receiver()) -> event => tray.click(event?, &actions),
            recv(updates) -> update => match update? {
                Update::State(state) => tray.apply(state),
                Update::Close => return Ok(()),
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn run() -> Result<(), Box<dyn Error>> {
    use tao::{
        event::{Event, StartCause},
        event_loop::{ControlFlow, EventLoopBuilder},
        platform::{
            macos::{ActivationPolicy, EventLoopExtMacOS},
            run_return::EventLoopExtRunReturn,
        },
    };
    enum UiEvent {
        Click(MenuEvent),
        Update(Update),
    }
    let mut event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |click| {
        let _ = proxy.send_event(UiEvent::Click(click));
    }));
    let proxy = event_loop.create_proxy();
    let (actions, requests) = channel::unbounded();
    let mut requests = Some(requests);
    let mut tray = None;
    let mut timer = None;
    let mut error = None;
    event_loop.run_return(|event, _, flow| match event {
        Event::NewEvents(StartCause::Init) => match Tray::new() {
            Ok(created) => {
                timer = Some(start_menu_timer(&created.info));
                tray = Some(created);
                let proxy = proxy.clone();
                session::spawn(requests.take().expect("first event"), move |update| {
                    let _ = proxy.send_event(UiEvent::Update(update));
                });
            }
            Err(failure) => {
                error = Some(failure);
                *flow = ControlFlow::Exit;
            }
        },
        Event::UserEvent(UiEvent::Click(click)) => {
            if let Some(tray) = &tray {
                tray.click(click, &actions);
            }
        }
        Event::UserEvent(UiEvent::Update(Update::State(state))) => {
            if let Some(tray) = &mut tray {
                tray.apply(state);
            }
        }
        Event::UserEvent(UiEvent::Update(Update::Close)) => *flow = ControlFlow::Exit,
        _ => {}
    });
    if let Some(timer) = timer {
        timer.invalidate();
    }
    error.map_or(Ok(()), Err)
}

#[cfg(target_os = "macos")]
fn start_menu_timer(info: &Rc<InfoRow>) -> objc2::rc::Retained<objc2_foundation::NSTimer> {
    use block2::RcBlock;
    use objc2_app_kit::NSEventTrackingRunLoopMode;
    use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoop, NSTimer};
    use std::rc::Weak;

    thread_local! {
        static INFO: RefCell<Weak<InfoRow>> = const { RefCell::new(Weak::new()) };
    }
    INFO.with(|slot| slot.replace(Rc::downgrade(info)));
    let tick = RcBlock::new(|_: std::ptr::NonNull<NSTimer>| {
        if let Some(info) = INFO.with(|slot| slot.borrow().upgrade()) {
            info.advance();
        }
    });
    // The block captures no state and the timer is registered on the main run loop.
    unsafe {
        let timer =
            NSTimer::timerWithTimeInterval_repeats_block(INTERVAL.as_secs_f64(), true, &tick);
        let run_loop = NSRunLoop::mainRunLoop();
        run_loop.addTimer_forMode(&timer, NSDefaultRunLoopMode);
        run_loop.addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
        timer
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn svg_renders_as_a_transparent_tray_icon() {
        let icon = super::make_icon();
        assert!(icon.is_ok());
        let tree = resvg::usvg::Tree::from_data(
            include_bytes!("icon.svg"),
            &resvg::usvg::Options::default(),
        )
        .unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(64, 64).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        assert_eq!(pixmap.pixel(0, 0).unwrap().alpha(), 0);
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() == 255));
    }
}
