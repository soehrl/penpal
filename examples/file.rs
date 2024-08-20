use anyhow::{bail, Result};
use penpal::{CollectionBox, Mailbox};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    Number(usize),
    String(String),
}

const FILE_NAME: &str = "penpal_file_example_communication_file.dat";

fn writer() -> Result<()> {
    println!("[writer] creating file {}", FILE_NAME);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(FILE_NAME)?;

    let collection_box = CollectionBox::new(file)?;

    let messages = [
        Message::Number(13),
        Message::String("foo".to_string()),
        Message::Number(141),
        Message::String("bar".to_string()),
        Message::Number(81),
        Message::String("baz".to_string()),
        Message::Number(78),
    ];

    for message in messages {
        println!("[writer] writing {:?}", message);
        collection_box.send(message)?;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    collection_box.farewell()?;

    // Wait until all messages are actually written.
    collection_box.wait()?;

    Ok(())
}

fn reader() -> Result<()> {
    println!("[reader] opening file {}", FILE_NAME);
    let file = std::fs::File::open(FILE_NAME)?;

    let mailbox = Mailbox::new(file);

    println!("[reader] reading messages");
    for message in mailbox {
        match message {
            Ok(Message::Number(n)) => println!("[reader] read number: {}", n),
            Ok(Message::String(s)) => println!("[reader] read string: {}", s),
            Err(penpal::ReceiveError::ReadError(e)) => {
                // This error is expected when dealing with partially written files
                if e.kind() == std::io::ErrorKind::UnexpectedEof {}
            }
            Err(e) => bail!(e),
        }
    }
    println!("[reader] read farewell");

    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);

    match args.next().as_ref().map(|s| s.as_str()) {
        Some("reader") => reader()?,
        Some("writer") => writer()?,
        Some(arg) => println!("expected 'reader' or 'writer', got '{}'", arg),
        None => {
            let writer = std::thread::spawn(writer);
            let reader = std::thread::spawn(|| {
                // Wait until the file is created
                std::thread::sleep(std::time::Duration::from_secs(1));
                reader()
            });

            reader.join().unwrap()?;
            writer.join().unwrap()?;
        }
    }

    Ok(())
}
