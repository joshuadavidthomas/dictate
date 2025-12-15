use std::fmt;

use crate::{
    dialogs::Reflection,
    time_block::*
};


pub struct PomodoroReport {
    pub time_block: TimeBlock,
    pub end: chrono::DateTime<chrono::Local>,
    pub intention: Option<String>,
    pub reflection: Reflection
}

impl PomodoroReport {
    pub fn new_from_time_block(time_block: TimeBlock, intention: Option<String>) -> Self
    {
        let now = chrono::offset::Local::now();

        Self {
            time_block,
            end: now,
            intention,
            reflection: Reflection::NoComment
        }
    }
}

impl fmt::Debug for PomodoroReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PomodoroReport")
            .field("start", &self.time_block.start)
            .field("end", &self.end)
            .field("intention", &self.intention)
            .field("reflection", &self.reflection)
            .finish()
    }
}
