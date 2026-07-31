mod cli;
mod daemon;
mod settings;

use std::path::Path;

use anyhow::Result;
use dictate_debug::Config as DebugConfig;
use dictate_ui::UiIdentity;

const UI_IDENTITY: UiIdentity = UiIdentity::new(
    env!("DICTATE_OVERLAY_APP_ID"),
    env!("DICTATE_OVERLAY_NAMESPACE"),
);

fn main() -> Result<()> {
    let debug_config = DebugConfig::new(
        env!("DICTATE_DEBUG_APP_ID"),
        env!("DICTATE_DISPLAY_NAME"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../dictate-speech/tests/fixtures"),
    );

    cli::run(UI_IDENTITY, debug_config)
}
