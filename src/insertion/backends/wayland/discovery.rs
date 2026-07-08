use std::time::Duration;
use std::time::Instant;

use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::Proxy;
use wayland_client::QueueHandle;
use wayland_client::WEnum;
use wayland_client::protocol::wl_callback;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;

use super::io::dispatch_until_event_or_timeout;
use crate::insertion::InsertionBackendKind;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionIoOperation;
use crate::insertion::InsertionTargetKind;

pub(super) struct DiscoveredInputMethod {
    pub(super) manager: ZwpInputMethodManagerV2,
    pub(super) seat: wl_seat::WlSeat,
}

struct DiscoveredSeat {
    proxy: wl_seat::WlSeat,
    has_keyboard: bool,
}

#[derive(Default)]
struct DiscoveryState {
    input_method_manager: Option<ZwpInputMethodManagerV2>,
    seats: Vec<DiscoveredSeat>,
    roundtrip_done: bool,
}

pub(super) fn discover_registry(
    connection: &Connection,
    idle_timeout: Duration,
) -> Result<DiscoveredInputMethod, InsertionFailure> {
    let mut event_queue = connection.new_event_queue();
    let qh = event_queue.handle();
    let mut state = DiscoveryState::default();

    connection.display().get_registry(&qh, ());
    wait_for_next_roundtrip(connection, &mut event_queue, &mut state, &qh, idle_timeout)?;

    if !state.seats.is_empty() {
        wait_for_next_roundtrip(connection, &mut event_queue, &mut state, &qh, idle_timeout)?;
    }

    let Some(manager) = state.input_method_manager else {
        return Err(InsertionFailure::BackendUnavailable {
            backend: InsertionBackendKind::WaylandInputMethod,
        });
    };

    let mut keyboard_seats = state.seats.into_iter().filter(|seat| seat.has_keyboard);
    let Some(first_seat) = keyboard_seats.next() else {
        return Err(InsertionFailure::TargetUnavailable {
            target: InsertionTargetKind::Seat,
        });
    };
    let remaining_count = keyboard_seats.count();
    if remaining_count > 0 {
        return Err(InsertionFailure::AmbiguousTarget {
            target: InsertionTargetKind::Seat,
            count: seat_count_for_report(remaining_count + 1),
        });
    }

    Ok(DiscoveredInputMethod {
        manager,
        seat: first_seat.proxy,
    })
}

fn wait_for_next_roundtrip(
    connection: &Connection,
    event_queue: &mut wayland_client::EventQueue<DiscoveryState>,
    state: &mut DiscoveryState,
    qh: &QueueHandle<DiscoveryState>,
    idle_timeout: Duration,
) -> Result<(), InsertionFailure> {
    state.roundtrip_done = false;
    connection.display().sync(qh, ());
    let deadline = Instant::now() + idle_timeout;

    while !state.roundtrip_done && Instant::now() < deadline {
        dispatch_until_event_or_timeout(connection, event_queue, state, deadline)?;
    }

    if state.roundtrip_done {
        Ok(())
    } else {
        Err(InsertionFailure::IdleTimedOut {
            operation: InsertionIoOperation::WaitReadable,
        })
    }
}

fn seat_count_for_report(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

impl Dispatch<wl_registry::WlRegistry, ()> for DiscoveryState {
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
            "wl_seat" => {
                let proxy = registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ());
                state.seats.push(DiscoveredSeat {
                    proxy,
                    has_keyboard: false,
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputMethodManagerV2, ()> for DiscoveryState {
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

impl Dispatch<wl_seat::WlSeat, ()> for DiscoveryState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities { capabilities } = event else {
            return;
        };
        let has_keyboard = match capabilities {
            WEnum::Value(capabilities) => capabilities.contains(wl_seat::Capability::Keyboard),
            WEnum::Unknown(_) => false,
        };
        if let Some(discovered) = state
            .seats
            .iter_mut()
            .find(|discovered| discovered.proxy.id() == seat.id())
        {
            discovered.has_keyboard = has_keyboard;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for DiscoveryState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.roundtrip_done = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_seat_count_reports_u32_max() {
        assert_eq!(seat_count_for_report(usize::MAX), u32::MAX);
    }
}
