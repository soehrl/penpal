// use std::io::Result;
use anyhow::Result;
use penpal::Transport;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
enum Message {
    Request(String),
    Response(String),
}

fn server() -> Result<()> {
    let listener = std::net::TcpListener::bind("127.0.0.1:8080")?;
    println!("Listening on {}", listener.local_addr()?);

    let (stream, _) = listener.accept()?;
    println!("Connected to {}", stream.peer_addr()?);

    let transport = Transport::from_tcp_stream(stream)?;
    let connection = transport.start_correspondence(|message, sender| match message {
        Message::Request(request) => {
            println!("server: received request: {}", request);

            // send the response
            sender.send(Message::Response("Hello, world!".to_string()))?;

            // but also also send a request
            sender.send(Message::Request("Hello, world!".to_string()))?;

            Ok(())
        }
        Message::Response(response) => {
            println!("server: received response: {}", response);

            // okay, we got our response, let's close the connection
            sender.farewell()?;
            
            Ok(())
        }
    })?;

    connection.wait()?;
    println!("server: connection closed");

    Ok(())
}

fn client() -> Result<()> {
    println!("Connecting to server...");
    let stream = std::net::TcpStream::connect("127.0.0.1:8080")?;
    println!("Connected to {}", stream.peer_addr()?);
    let transport = Transport::from_tcp_stream(stream)?;

    let connection =
        transport.start_correspondence_with_association(vec![], |message, state, sender| {
            match message {
                Message::Request(request) => {
                    println!("client: received request: {}", request);
                    sender.send(Message::Response("Hello, world!".to_string()))?;
                    Ok(())
                }
                Message::Response(response) => {
                    println!("client: received response: {}", response);

                    // let's memorize the responses
                    state.write().unwrap().push(response);

                    Ok(())
                }
            }
        })?;

    connection.send(Message::Request("Hello, world!".to_string()))?;
    connection.wait()?;

    let responses = connection.association().read().unwrap();
    println!("client: received responses: {:?}", responses);


    Ok(())
}

fn main() -> Result<()> {
    let server = std::thread::spawn(server);
    let client = std::thread::spawn(client);

    client.join().unwrap()?;
    server.join().unwrap()?;

    Ok(())
}
