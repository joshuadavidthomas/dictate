use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use raw_window_handle::HasDisplayHandle;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawDisplayHandle;
use raw_window_handle::RawWindowHandle;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::Proxy;
use wayland_client::QueueHandle;
use wayland_client::backend::Backend;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

#[derive(Default)]
struct ActivationState {
    activation: Option<XdgActivationV1>,
}

pub fn activate_window(
    window: &(impl HasDisplayHandle + HasWindowHandle),
    token: &str,
) -> Result<()> {
    if token.is_empty() {
        bail!("the Wayland activation token is empty");
    }

    let display_handle = window.display_handle()?;
    let RawDisplayHandle::Wayland(display_handle) = display_handle.as_raw() else {
        bail!("the window is not using Wayland");
    };
    let window_handle = window.window_handle()?;
    let RawWindowHandle::Wayland(window_handle) = window_handle.as_raw() else {
        bail!("the window is not using Wayland");
    };

    // SAFETY: The caller's window owns the display pointer for its lifetime, which outlives the
    // guest backend and every proxy created below. The guest backend does not close it on drop.
    let backend = unsafe { Backend::from_foreign_display(display_handle.display.as_ptr().cast()) };
    let connection = Connection::from_backend(backend);
    // SAFETY: The caller's window owns this wl_surface pointer. The imported ID and proxy are used
    // only in this function and belong to the display imported immediately above.
    let surface_id = unsafe {
        ObjectId::from_ptr(
            WlSurface::interface(),
            window_handle.surface.as_ptr().cast(),
        )
    }
    .context("failed to import the Wayland surface")?;
    let surface = WlSurface::from_id(&connection, surface_id)
        .context("failed to create a proxy for the Wayland surface")?;

    let mut queue = connection.new_event_queue();
    let queue_handle = queue.handle();
    connection.display().get_registry(&queue_handle, ());

    let mut state = ActivationState::default();
    queue
        .roundtrip(&mut state)
        .context("failed to discover Wayland activation support")?;
    let activation = state
        .activation
        .context("the Wayland compositor does not support xdg-activation-v1")?;

    activation.activate(token.to_owned(), &surface);
    activation.destroy();
    connection
        .flush()
        .context("failed to send the Wayland activation request")?;
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, ()> for ActivationState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "xdg_activation_v1"
        {
            state.activation = Some(registry.bind::<XdgActivationV1, (), Self>(
                name,
                version.min(1),
                queue_handle,
                (),
            ));
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for ActivationState {
    fn event(
        _state: &mut Self,
        _activation: &XdgActivationV1,
        _event: xdg_activation_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}
