use std::process::ExitStatus;
use std::io;

use futures_util::{
    FutureExt
};

use tokio::process::Command;

use relm4::{
    Worker,
    ComponentSender
};


pub struct Subprocess;

#[derive(Debug)]
pub enum SubprocessMessage {
    Done(io::Result<ExitStatus>),
}

impl Worker for Subprocess {
    type Init = String;
    type Input = ();

    type Output = SubprocessMessage;

    fn init(cmdline: Self::Init, sender: ComponentSender<Self>) -> Self
    {
        let fut = {
            let output = sender.output_sender().clone();

            async move {
                let cmd = Command::new(cmdline)
                    .kill_on_drop(true)
                    .status()
                    .map(SubprocessMessage::Done);

                let status = cmd.await;
                _ = output.send(status);
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

