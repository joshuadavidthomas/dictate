mod cli;
mod daemon;
mod settings;

use anyhow::Result;
use dictate_ui::UiIdentity;

const UI_IDENTITY: UiIdentity = UiIdentity::new(
    env!("DICTATE_OVERLAY_APP_ID"),
    env!("DICTATE_OVERLAY_NAMESPACE"),
);

fn main() -> Result<()> {
    cli::run(UI_IDENTITY)
}
