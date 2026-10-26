mod cli;
mod config_edit;
mod daemon;
mod install_cli;
mod launch;
mod settings;

use anyhow::Result;
use dictate_ui::UiIdentity;

const UI_IDENTITY: UiIdentity = UiIdentity::new(
    env!("DICTATE_PORTAL_APP_ID"),
    env!("DICTATE_OVERLAY_APP_ID"),
    env!("DICTATE_OVERLAY_NAMESPACE"),
);

fn main() -> Result<()> {
    // GPUI and other dependencies report problems through `log`; without a logger they vanish.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    cli::run(UI_IDENTITY)
}
