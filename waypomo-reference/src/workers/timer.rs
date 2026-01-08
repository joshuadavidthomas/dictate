use std::time::Duration;

use tokio::time;

use relm4::{
    Worker,
    ComponentSender
};


pub struct Timer;

#[derive(Debug)]
pub enum TimerMessage {
    Tick,
}

impl Worker for Timer {
    type Init = Duration;
    type Input = ();

    type Output = TimerMessage;

    fn init(duration: Self::Init, sender: ComponentSender<Self>) -> Self
    {
        let fut = {
            let output = sender.output_sender().clone();

            async move {
                let mut interval = time::interval(duration);

                interval.tick().await;

                loop {
                    output.send(TimerMessage::Tick).unwrap();
                    interval.tick().await;
                }
            }
        };

        sender.command(move |_, shutdown| {
            shutdown.register(fut)
                .drop_on_shutdown()
        });

        Self
    }

    fn update(&mut self, _: Self::Input, _: ComponentSender<Self>)
    {
    }
}
