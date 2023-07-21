use anyhow::Result;
use rustyline::{error::ReadlineError, history::DefaultHistory, Editor};
use std::sync::mpsc::Receiver as StdReceiver;
use tokio::sync::mpsc::Sender as TokioSender;

pub async fn run_readline_loop(
    sender: TokioSender<String>,
    interrupt_sender: TokioSender<()>,
    receiver: StdReceiver<()>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut editor = Editor::<(), DefaultHistory>::new()?;
        loop {
            match editor.readline("> ") {
                Ok(line) => {
                    editor.add_history_entry(line.as_str()).unwrap();
                    sender.blocking_send(line).unwrap();
                }
                Err(ReadlineError::Interrupted) => {
                    interrupt_sender.blocking_send(()).unwrap();
                }
                Err(ReadlineError::Eof) => break,
                Err(err) => return Err(err.into()),
            }

            if receiver.recv().is_err() {
                break;
            }
        }
        Ok(())
    })
    .await?
}
