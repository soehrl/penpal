use anyhow::{anyhow, Result};
use penpal::Transport;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
enum Request {
    CalcLength(String),
}

#[derive(Serialize, Deserialize)]
enum Response {
    StringLength(usize),
}

const ADDRESS: &str = "127.0.0.1:8080";

fn server() -> Result<()> {
    let listener = std::net::TcpListener::bind(ADDRESS)?;
    println!("[server] listening on {}", listener.local_addr()?);

    let (stream, _) = listener.accept()?;
    println!("[server] new client: {}", stream.peer_addr()?);

    let transport = Transport::from_tcp_stream(stream)?;
    let (mailbox, collection_box) = transport.create_infrastructue::<Request, Response>()?;

    println!("[server] waiting for messages");
    for message in mailbox {
        match message? {
            Request::CalcLength(str) => {
                println!("[server] calculating length of '{}'", str);
                let l = str.len();

                println!("[server] sending {} to client", l);
                collection_box.send(Response::StringLength(l))?;
            }
        }
    }
    println!("[server] client said farewell");

    Ok(())
}

fn client() -> Result<()> {
    println!("[client] connecting to {}", ADDRESS);
    let stream = std::net::TcpStream::connect(ADDRESS)?;
    println!("[client] connected to {}", stream.peer_addr()?);
    let transport = Transport::from_tcp_stream(stream)?;

    let (mut mailbox, collection_box) = transport.create_infrastructue::<Response, Request>()?;

    for word in ["Hello", "beautiful", "World"] {
        println!("[client] requesting server to calculate length of {}", word);
        collection_box.send(Request::CalcLength(word.to_string()))?;
        match mailbox
            .receive()?
            .ok_or_else(|| anyhow!("unexpected farewell from server"))?
        {
            Response::StringLength(l) => {
                println!("[client] server responded with: {}", l);
            }
        }
    }

    println!("[client] saying farewell to server");
    collection_box.farewell()?;

    Ok(())
}

fn main() -> Result<()> {
    let server = std::thread::spawn(server);
    let client = std::thread::spawn(client);

    client.join().unwrap()?;
    server.join().unwrap()?;

    Ok(())
}
