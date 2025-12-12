#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::option_map_unit_fn,
    clippy::wildcard_imports,
    clippy::ignored_unit_patterns,
    clippy::enum_glob_use,
)]

use std::mem;

use relm4::prelude::*;

mod cmdline;
mod config;
mod dialogs;
mod report;
mod time_block;
mod waypomo;
mod workers;

use cmdline::*;
use waypomo::*;


fn main() {
    let mut args: WaypomoArgs = argh::from_env();
    let conf = {
        let mut conf = config::load(args.config.as_deref());

        if let Some(start_in) = args.start_in {
            conf.pomodoro.start_in = start_in;
        }

        conf
    };

    let gtk_args = {
        let mut a = mem::take(&mut args.rest);
        a.insert(0, std::env::args().next().unwrap());
        a
    };

    #[cfg(debug_assertions)]
    let app = RelmApp::new("sh.wrl.waypomo-debug");
    #[cfg(not(debug_assertions))]
    let app = RelmApp::new("sh.wrl.waypomo");

    relm4::set_global_css(include_str!("../config/style.css"));

    app.with_args(gtk_args)
        .run::<Waypomo>(conf);
}
