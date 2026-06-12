use anyhow::Result;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::QueueHandle;
use wayland_client::globals::GlobalListContents;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_registry;

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn main() -> Result<()> {
    let connection = Connection::connect_to_env()?;
    let (globals, _event_queue) = registry_queue_init::<State>(&connection)?;
    let mut globals = globals.contents().clone_list();
    globals.sort_by(|left, right| {
        left.interface
            .cmp(&right.interface)
            .then(left.name.cmp(&right.name))
    });

    for global in globals {
        println!(
            "{} v{} name={}",
            global.interface, global.version, global.name
        );
    }

    Ok(())
}
