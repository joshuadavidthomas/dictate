use std::io::prelude::*;
use std::path::PathBuf;
use std::str::FromStr;
use std::fs::File;

use chrono::TimeDelta;

use directories::ProjectDirs;


fn time_delta_decode(s: &str) -> Result<TimeDelta, humantime::DurationError>
{
    humantime::Duration::from_str(s)
        .map(|d| TimeDelta::from_std(d.into())
                .unwrap())
}

#[derive(Debug, Clone, knus::Decode)]
pub struct WaypomoConfig {
    #[knus(child, default)]
    pub pomodoro: PomodoroConfig,

    #[knus(child, default)]
    pub display: DisplayConfig,
}

const WORK_DEFAULT: TimeBlockConfig = TimeBlockConfig {
    duration: time_delta_for_minutes(25),
    on_completion: None
};

const SHORT_BREAK_DEFAULT: TimeBlockConfig = TimeBlockConfig {
    duration: time_delta_for_minutes(5),
    on_completion: None
};

const LONG_BREAK_DEFAULT: TimeBlockConfig = TimeBlockConfig {
    duration: time_delta_for_minutes(15),
    on_completion: None
};

#[derive(Debug, Clone, knus::Decode)]
pub struct PomodoroConfig {
    #[knus(
        child,
        default = WORK_DEFAULT
    )]
    pub work: TimeBlockConfig,

    #[knus(
        child,
        default = SHORT_BREAK_DEFAULT
    )]
    pub short_break: TimeBlockConfig,

    #[knus(
        child,
        default = LONG_BREAK_DEFAULT
    )]
    pub long_break: TimeBlockConfig,

    #[knus(
        child,
        unwrap(argument),
        default = 4
    )]
    pub work_blocks_for_long_break: usize,

    #[knus(
        child,
        unwrap(argument, decode_with = time_delta_decode)
    )]
    pub reflection_prompt_delay: Option<TimeDelta>,

    #[knus(
        child,
        unwrap(argument),
        default=false
    )]
    pub motivation_dialog_after_intentionless_block: bool,

    #[knus(
        child,
        unwrap(argument),
        default=StartIn::ShortBreak
    )]
    pub start_in: StartIn,
}

impl Default for PomodoroConfig
{
    fn default() -> Self
    {
        Self {
            work: WORK_DEFAULT,
            short_break: SHORT_BREAK_DEFAULT,
            long_break: LONG_BREAK_DEFAULT,

            work_blocks_for_long_break: 4,
            reflection_prompt_delay: None,
            motivation_dialog_after_intentionless_block: true,
            start_in: StartIn::ShortBreak
        }
    }
}

#[derive(Debug, Clone, knus::DecodeScalar)]
pub enum StartIn {
    Work,
    ShortBreak,
    LongBreak
}

impl FromStr for StartIn {
    type Err = &'static str;

    fn from_str(x: &str) -> Result<Self, Self::Err>
    {
        let res = match x {
            "work" => Self::Work,
            "short" | "short break" | "short-break" => Self::ShortBreak,
            "long" | "lomg" | "long break" | "long-break" => Self::LongBreak,
            _ => return Err("unrecognised mode")
        };

        Ok(res)
    }
}

#[derive(Debug, Clone, knus::Decode)]
pub struct TimeBlockConfig {
    #[knus(
        argument,
        decode_with = time_delta_decode
    )]
    pub duration: TimeDelta,

    #[knus(
        child,
        unwrap(argument)
    )]
    pub on_completion: Option<String>
}

#[derive(Debug, Clone, Copy, knus::DecodeScalar)]
pub enum TimerPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight
}

#[derive(Debug, Clone, Copy)]
pub struct PositionAnchors {
    pub top: bool,
    pub bottom: bool,
    pub right: bool,
    pub left: bool
}

impl From<(bool, bool, bool, bool)> for PositionAnchors
{
    #[inline]
    fn from((top, bottom, right, left): (bool, bool, bool, bool)) -> Self
{
        Self {
            top,
            bottom,
            right,
            left,
        }
    }
}

impl TimerPosition {
    #[inline]
    pub fn anchors(&self) -> PositionAnchors
    {
        use TimerPosition::*;

        match self {
            TopLeft     => (true,  false, false, true),
            TopRight    => (true,  false, true,  false),
            BottomLeft  => (false, true,  false, true),
            BottomRight => (false, true,  true,  false),
        }.into()
    }
}

#[derive(Debug, Clone, knus::Decode)]
pub struct DisplayConfig {
    #[knus(
        child,
        unwrap(argument),
        default = false
    )]
    pub dark_mode: bool,

    #[knus(
        child,
        unwrap(argument),
        default = TimerPosition::BottomRight
    )]
    pub timer_position: TimerPosition
}

impl Default for DisplayConfig
{
    fn default() -> Self
    {
        Self {
            dark_mode: false,
            timer_position: TimerPosition::BottomRight
        }
    }
}

#[inline]
const fn time_delta_for_minutes(minutes: i64) -> TimeDelta
{
    TimeDelta::new(minutes * 60, 0)
        .unwrap()
}

impl Default for WaypomoConfig
{
    fn default() -> Self
    {
        Self {
            pomodoro: PomodoroConfig::default(),
            display: DisplayConfig::default(),
        }
    }
}

pub fn load(path: Option<&str>) -> WaypomoConfig
{
    let conf = {
        let path = path
            .map(Into::<PathBuf>::into)
            .or_else(|| {
                ProjectDirs::from("sh", "wrl", "waypomo")
                    .map(|d| d.config_dir().join("config.kdl"))
            });

        path
            .and_then(|p| File::open(p).ok())
            .and_then(|mut f| {
                let mut contents = String::new();
                f.read_to_string(&mut contents).ok()
                    .map(move |_| contents)
            })
    };

    let Some(conf) = conf else {
        return WaypomoConfig::default();
    };

    match knus::parse::<WaypomoConfig>("config.kdl", &conf) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{:?}", miette::Report::new(e));
            std::process::exit(1);
        }
    }
}
