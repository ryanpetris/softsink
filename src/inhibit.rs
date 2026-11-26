use std::sync::mpsc::Receiver;
use std::time::Duration;

use zbus::{Connection, block_on, connection::Builder};

const SERVICE: &str = "org.freedesktop.ScreenSaver";
const PATH: &str = "/org/freedesktop/ScreenSaver";

struct Inhibitor<'a> {
    connection: &'a Connection,
    cookie: u32,
}

impl<'a> Inhibitor<'a> {
    fn acquire(connection: &'a Connection) -> zbus::Result<Self> {
        let cookie = block_on(connection.call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "Inhibit",
            &("Softsink", "Showing video"),
        ))?
        .body()
        .deserialize()?;
        Ok(Self { connection, cookie })
    }
}

impl Drop for Inhibitor<'_> {
    fn drop(&mut self) {
        let _ = block_on(self.connection.call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "UnInhibit",
            &(self.cookie,),
        ));
    }
}

pub fn run(states: Receiver<bool>) {
    let connection = match block_on(
        Builder::session()
            .method_timeout(Duration::from_secs(2))
            .build(),
    ) {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("Could not inhibit screen blanking: {error}");
            return;
        }
    };
    let mut inhibitor = None;
    while let Ok(running) = states.recv() {
        if running && inhibitor.is_none() {
            match Inhibitor::acquire(&connection) {
                Ok(held) => inhibitor = Some(held),
                Err(error) => eprintln!("Could not inhibit screen blanking: {error}"),
            }
        } else if !running {
            inhibitor = None;
        }
    }
}
