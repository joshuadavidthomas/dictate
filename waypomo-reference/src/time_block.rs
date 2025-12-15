use crate::config::TimeBlockConfig;


// just something so that the timer doesn't start *immediately*.
// feels a little better.
const LEAD_IN: chrono::TimeDelta =
    chrono::TimeDelta::new(1, 250_000_000).unwrap();

#[derive(Clone)]
pub struct TimeBlock {
    pub conf: TimeBlockConfig,
    pub remaining: chrono::TimeDelta,
    pub start: chrono::DateTime<chrono::Local>,
}

impl TimeBlock {
    pub fn new_with_conf(conf: TimeBlockConfig) -> Self
    {
        let remaining = conf.duration + LEAD_IN;
        let now = chrono::offset::Local::now();

        Self {
            conf,
            remaining,
            start: now
        }
    }

    pub fn reset(&mut self)
    {
        self.remaining = self.conf.duration + LEAD_IN;
    }
}
