use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::QueueHandle;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;

const DEFAULT_TEXT: &str = "hello from dictate ";
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> Result<()> {
    let text = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_TEXT.to_owned());

    let connection = Connection::connect_to_env()?;
    let mut event_queue = connection.new_event_queue();
    let qh = event_queue.handle();

    connection.display().get_registry(&qh, ());

    let mut state = State::new(text);
    event_queue.roundtrip(&mut state)?;

    let manager = state
        .input_method_manager
        .as_ref()
        .context("zwp_input_method_manager_v2 is not advertised by this compositor")?
        .clone();
    let seat = state
        .seat
        .as_ref()
        .context("wl_seat is not advertised by this compositor")?
        .clone();

    state.input_method = Some(manager.get_input_method(&seat, &qh, ()));
    event_queue.roundtrip(&mut state)?;

    eprintln!(
        "input-method prototype ready; focus a text field within {}s",
        WAIT_TIMEOUT.as_secs()
    );

    let deadline = Instant::now() + WAIT_TIMEOUT;
    while !state.progress.is_finished() && Instant::now() < deadline {
        event_queue.roundtrip(&mut state)?;
        std::thread::sleep(Duration::from_millis(100));
    }

    match state.progress {
        InputMethodProgress::Committed => Ok(()),
        InputMethodProgress::Unavailable => {
            bail!("input method unavailable; another input method likely owns the seat")
        }
        InputMethodProgress::Waiting | InputMethodProgress::Active => {
            bail!("timed out before an active text input accepted the input method")
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputMethodProgress {
    Waiting,
    Active,
    Committed,
    Unavailable,
}

impl InputMethodProgress {
    const fn is_finished(self) -> bool {
        matches!(self, Self::Committed | Self::Unavailable)
    }
}

struct State {
    input_method_manager: Option<ZwpInputMethodManagerV2>,
    input_method: Option<ZwpInputMethodV2>,
    seat: Option<wl_seat::WlSeat>,
    text: String,
    progress: InputMethodProgress,
    serial: u32,
}

impl State {
    fn new(text: String) -> Self {
        Self {
            input_method_manager: None,
            input_method: None,
            seat: None,
            text,
            progress: InputMethodProgress::Waiting,
            serial: 0,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version: _,
        } = event
        else {
            return;
        };

        match interface.as_str() {
            "zwp_input_method_manager_v2" => {
                state.input_method_manager =
                    Some(registry.bind::<ZwpInputMethodManagerV2, _, _>(name, 1, qh, ()));
            }
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputMethodManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        input_method: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        (): &(),
        connection: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                eprintln!("input method activated");
                state.progress = InputMethodProgress::Active;
            }
            zwp_input_method_v2::Event::Deactivate => {
                eprintln!("input method deactivated");
                state.progress = InputMethodProgress::Waiting;
            }
            zwp_input_method_v2::Event::Done => {
                state.serial += 1;

                if state.progress == InputMethodProgress::Active {
                    input_method.commit_string(state.text.clone());
                    input_method.commit(state.serial);
                    if let Err(error) = connection.flush() {
                        eprintln!("failed to flush commit_string request: {error}");
                    }
                    eprintln!("committed {:?} with serial {}", state.text, state.serial);
                    state.progress = InputMethodProgress::Committed;
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                eprintln!("input method unavailable");
                state.progress = InputMethodProgress::Unavailable;
            }
            zwp_input_method_v2::Event::SurroundingText { .. }
            | zwp_input_method_v2::Event::TextChangeCause { .. }
            | zwp_input_method_v2::Event::ContentType { .. }
            | _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
