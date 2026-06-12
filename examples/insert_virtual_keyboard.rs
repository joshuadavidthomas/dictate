use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::os::fd::AsFd;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::QueueHandle;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use xkbcommon::xkb;

const DEFAULT_TEXT: &str = "hello from dictate ";
const KEYMAP_FORMAT_XKB_V1: u32 = 1;
const KEY_RELEASED: u32 = 0;
const KEY_PRESSED: u32 = 1;
const XKB_OFFSET: u32 = 8;

fn main() -> Result<()> {
    let text = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_TEXT.to_owned());

    let connection = Connection::connect_to_env()?;
    let mut event_queue = connection.new_event_queue();
    let qh = event_queue.handle();

    connection.display().get_registry(&qh, ());

    let mut state = State::default();
    event_queue.roundtrip(&mut state)?;

    let manager = state
        .virtual_keyboard_manager
        .as_ref()
        .context("zwp_virtual_keyboard_manager_v1 is not advertised by this compositor")?
        .clone();
    let seat = state
        .seat
        .as_ref()
        .context("wl_seat is not advertised by this compositor")?
        .clone();
    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    state.virtual_keyboard = Some(keyboard.clone());

    let keymap = CharacterKeymap::new(&text)?;
    let keymap_file = write_keymap(&keymap.xkb_source)?;
    keyboard.keymap(
        KEYMAP_FORMAT_XKB_V1,
        keymap_file.as_fd(),
        keymap.xkb_source.len() as u32,
    );
    event_queue.roundtrip(&mut state)?;

    eprintln!("typing {:?} with virtual keyboard", text);
    for character in text.chars() {
        let key = keymap
            .key_for(character)
            .with_context(|| format!("character missing from generated keymap: {character:?}"))?;
        keyboard.key(0, key, KEY_PRESSED);
        keyboard.key(0, key, KEY_RELEASED);
    }
    connection.flush()?;
    event_queue.roundtrip(&mut state)?;
    std::thread::sleep(Duration::from_millis(100));

    Ok(())
}

#[derive(Default)]
struct State {
    virtual_keyboard_manager: Option<ZwpVirtualKeyboardManagerV1>,
    virtual_keyboard: Option<ZwpVirtualKeyboardV1>,
    seat: Option<wl_seat::WlSeat>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
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
            "zwp_virtual_keyboard_manager_v1" => {
                state.virtual_keyboard_manager =
                    Some(registry.bind::<ZwpVirtualKeyboardManagerV1, _, _>(name, 1, qh, ()))
            }
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()))
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: zwp_virtual_keyboard_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: zwp_virtual_keyboard_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);

struct CharacterKeymap {
    xkb_source: String,
    key_by_char: BTreeMap<char, u32>,
}

impl CharacterKeymap {
    fn new(text: &str) -> Result<Self> {
        let mut key_by_char = BTreeMap::new();
        let mut keys = Vec::new();

        for character in text.chars() {
            if key_by_char.contains_key(&character) {
                continue;
            }

            let key = (keys.len() + 1)
                .try_into()
                .context("too many unique characters for generated keymap")?;
            let keysym = xkb::utf32_to_keysym(character as u32);
            if keysym.raw() == xkb::keysyms::KEY_NoSymbol {
                anyhow::bail!("no XKB keysym for character {character:?}");
            }
            let keysym_name = xkb::keysym_get_name(keysym);
            if keysym_name.is_empty() {
                anyhow::bail!("no XKB keysym name for character {character:?}");
            }

            key_by_char.insert(character, key);
            keys.push((key, keysym_name));
        }

        let xkb_source = build_xkb_source(&keys);
        Ok(Self {
            xkb_source,
            key_by_char,
        })
    }

    fn key_for(&self, character: char) -> Option<u32> {
        self.key_by_char.get(&character).copied()
    }
}

fn build_xkb_source(keys: &[(u32, String)]) -> String {
    let maximum = keys.len() as u32 + XKB_OFFSET + 1;
    let mut source = String::from("xkb_keymap {\n");

    source.push_str("xkb_keycodes \"(unnamed)\" {\n");
    source.push_str("minimum = 8;\n");
    source.push_str(&format!("maximum = {maximum};\n"));
    for (key, _) in keys {
        source.push_str(&format!("<K{key}> = {};\n", key + XKB_OFFSET));
    }
    source.push_str("};\n");

    source.push_str("xkb_types \"(unnamed)\" { include \"complete\" };\n");
    source.push_str("xkb_compatibility \"(unnamed)\" { include \"complete\" };\n");

    source.push_str("xkb_symbols \"(unnamed)\" {\n");
    for (key, keysym_name) in keys {
        source.push_str(&format!("key <K{key}> {{[{keysym_name}]}};\n"));
    }
    source.push_str("};\n");

    source.push_str("};\n\0");
    source
}

fn write_keymap(source: &str) -> Result<File> {
    let path = std::env::temp_dir().join(format!(
        "dictate-virtual-keyboard-keymap-{}.xkb",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("failed to create {path:?}"))?;
    file.write_all(source.as_bytes())?;
    file.seek(SeekFrom::Start(0))?;
    fs::remove_file(&path).with_context(|| format!("failed to unlink {path:?}"))?;
    Ok(file)
}
