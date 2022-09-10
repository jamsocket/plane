use anyhow::Result;
use signal_hook::{consts::SIGINT, iterator::Signals};
use std::thread;

async fn controller_main() -> Result<()> {


    Ok(())
}

pub fn run() -> Result<()> {
    let mut signals = Signals::new(&[SIGINT])?;

    thread::spawn(move || {
        for _ in signals.forever() {
            // TODO: we could shut down containers here.
            std::process::exit(0)
        }
    });

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(controller_main())?;

    Ok(())
}
