use argh::FromArgs;

use crate::config::*;


#[derive(FromArgs)]
/// endless pomodoros (pomodoroes? pomodori?)
pub struct WaypomoArgs {
    /// path to configuration file
    #[argh(option, short = 'c')]
    pub config: Option<String>,

    /// mode to start in (overrides config file)
    #[argh(option, short = 's')]
    pub start_in: Option<StartIn>,

    #[argh(positional, greedy)]
    pub rest: Vec<String>,
}
