use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    sync::{Arc, Condvar, Mutex, RwLock},
};
use thiserror::Error;

// enum Response<O: Serialize> {
//     None,
//     Single(O),
//     Multiple(Vec<O>),
//     Farewell,
// }

// impl<O: Serialize> Response<O> {
//     fn none() -> Self {
//         Self::None
//     }

//     fn single(message: O) -> Self {
//         Self::Single(message)
//     }

//     fn multiple(messages: impl Into<Vec<O>>) -> Self {
//         Self::Multiple(messages.into())
//     }

//     fn farewell() -> Self {
//         Self::Farewell
//     }

//     fn is_none(&self) -> bool {
//         matches!(self, Response::None)
//     }

//     fn is_farewell(&self) -> bool {
//         matches!(self, Response::Farewell)
//     }
// }

#[derive(Error, Debug, Clone)]
pub enum ReceiveError {
    #[error("failed to read from stream")]
    ReadError(Arc<std::io::Error>),
    #[error("failed to deserialize message")]
    DeserializeError(#[from] postcard::Error),
}

impl From<std::io::Error> for ReceiveError {
    fn from(err: std::io::Error) -> Self {
        Self::ReadError(Arc::new(err))
    }
}

fn read_until_error<R: Read>(reader: &mut R, buffer: &mut [u8]) -> std::io::Result<()> {
    let mut bytes_read = 0;
    while bytes_read < buffer.len() {
        match reader.read(&mut buffer[bytes_read..]) {
            Ok(n) => bytes_read += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                return Err(e);
            }
        }
    }
    Ok(())
}

fn receive<I: for<'de> Deserialize<'de>>(
    mut read: impl Read,
    buffer: &mut Vec<u8>,
) -> Result<Option<I>, ReceiveError> {
    let mut header = [0; 4];
    read_until_error(&mut read, &mut header)?;
    let payload_length = u32::from_le_bytes(header) as usize;
    buffer.resize(payload_length, 0);
    read_until_error(&mut read, buffer)?;
    let message = postcard::from_bytes(&buffer)?;
    Ok(message)
}

#[derive(Error, Debug, Clone)]
pub enum DeliveryError {
    #[error("failed to serialize message")]
    SerializeError(#[from] postcard::Error),
    #[error("failed to write to stream")]
    WriteError(Arc<std::io::Error>),
}

impl From<std::io::Error> for DeliveryError {
    fn from(err: std::io::Error) -> Self {
        Self::WriteError(Arc::new(err))
    }
}

fn send<O: Serialize>(
    mut write: impl Write,
    message: Option<&O>,
    buffer: &mut Vec<u8>,
) -> Result<(), DeliveryError> {
    struct Writer<'a>(&'a mut Vec<u8>);
    impl Extend<u8> for Writer<'_> {
        fn extend<T: IntoIterator<Item = u8>>(&mut self, iter: T) {
            self.0.extend(iter);
        }
    }
    buffer.resize(4, 0);
    postcard::to_extend(&message, Writer(buffer))?;
    let message_length = buffer.len();
    let payload_length = message_length as u32 - 4;
    let header = payload_length.to_le_bytes();
    buffer[0..4].copy_from_slice(&header);
    write.write_all(&buffer)?;
    Ok(())
}

/// Defines how the messages are transported.
///
/// Messages to and from the penpal can use different means of transportation.
pub struct Transport<Incoming: Read, Outgoing: Write> {
    incoming: Incoming,
    outgoing: Outgoing,
}

impl<IncomingTransport: Read, OutgoingTransport: Write>
    Transport<IncomingTransport, OutgoingTransport>
{
    /// Create a new penpal that sends and receives messages using the given transports.
    pub fn new(incoming: IncomingTransport, outgoing: OutgoingTransport) -> Self {
        Self { incoming, outgoing }
    }

    // fn correspond<I: for<'de> Deserialize<'de>, O: Serialize>(
    //     self,
    //     mut f: impl FnMut(I) -> Response<O>,
    // ) {
    //     let mut buffer = Vec::with_capacity(1024);
    //     let Self {
    //         mut incoming,
    //         mut outgoing,
    //     } = self;

    //     while let Some(message) = receive(&mut incoming, &mut buffer).unwrap() {
    //         match f(message) {
    //             Response::None => {}
    //             Response::Single(message) => {
    //                 send(&mut outgoing, Some(&message), &mut buffer).unwrap();
    //             }
    //             Response::Multiple(messages) => {
    //                 for message in messages {
    //                     send(&mut outgoing, Some(&message), &mut buffer).unwrap();
    //                 }
    //             }
    //             Response::Farewell => {
    //                 send::<O>(&mut outgoing, None, &mut buffer).unwrap();
    //                 break;
    //             }
    //         }
    //     }
    // }
}

impl<T: Read + Write + Clone> Transport<T, T> {
    /// Create a new penpal that sends and receives messages using the given transport.
    pub fn from_bidirectional(transport: T) -> Self {
        Self::new(transport.clone(), transport)
    }
}

impl Transport<std::net::TcpStream, std::net::TcpStream> {
    /// Creates a new penpal from a single TcpStream. It will use `stream.try_clone()` to create
    /// the incoming and outgoing transports.
    pub fn from_tcp_stream(stream: std::net::TcpStream) -> std::io::Result<Self> {
        let incoming_transport = stream.try_clone()?;
        let outgoing_transport = stream;
        Ok(Self::new(incoming_transport, outgoing_transport))
    }
}

#[derive(Error, Debug, Clone)]
#[error("sending a message has failed")]
pub enum CollectionBoxError {
    DeliveryError(#[from] DeliveryError),
    FarewellAlreadySent,
}

/// A collection box streamlines the sending of messages.
///
/// The collection box can be cloned allowing you to send messages of a specify type from multiple
/// places and threads. The collection box will forward the messages to an inaccessible `post
/// office` in a separate thread. The `post office` will then send the messages to the penpal using
/// the outgoing transport. Thus, the outgoing transport and the outgoing message typ must be
/// `Send`.
pub struct CollectionBox<T: Serialize> {
    post_office: std::sync::mpsc::Sender<Option<T>>,
    result: Arc<(Mutex<Option<Result<(), DeliveryError>>>, Condvar)>,
}

impl<T: Serialize> Clone for CollectionBox<T> {
    fn clone(&self) -> Self {
        Self {
            post_office: self.post_office.clone(),
            result: self.result.clone(),
        }
    }
}

impl<T: Serialize> CollectionBox<T> {
    fn post_office(
        collection_boxes: std::sync::mpsc::Receiver<Option<T>>,
        mut writer: impl Write,
        result: Arc<(Mutex<Option<Result<(), DeliveryError>>>, Condvar)>,
    ) {
        let mut buffer = Vec::with_capacity(1024);
        for message in collection_boxes {
            if let Err(err) = send(&mut writer, message.as_ref(), &mut buffer) {
                let (result, cvar) = &*result;
                let mut result = result.lock().unwrap();
                *result = Some(Err(err));
                cvar.notify_all();
                break;
            }

            if message.is_none() {
                // Farewell message
                let (result, cvar) = &*result;
                let mut result = result.lock().unwrap();
                *result = Some(Ok(()));
                cvar.notify_all();
                break;
            }
        }
    }

    pub fn wait(&self) -> Result<(), DeliveryError> {
        let (result, cvar) = &*self.result;
        let mut result = result.lock().unwrap();

        loop {
            if let Some(result) = &*result {
                return result.clone();
            }
            result = cvar.wait(result).unwrap();
        }
    }

    pub fn available(&self) -> bool {
        self.result.0.lock().unwrap().is_none()
    }

    pub fn send(&self, postcard: T) -> Result<(), CollectionBoxError> {
        self.send_impl(Some(postcard))
    }

    pub fn farewell(&self) -> Result<(), CollectionBoxError> {
        self.send_impl(None)
    }

    fn send_impl(&self, message: Option<T>) -> Result<(), CollectionBoxError> {
        if let Err(_) = self.post_office.send(message) {
            match self.wait() {
                Ok(()) => return Err(CollectionBoxError::FarewellAlreadySent),
                Err(err) => return Err(CollectionBoxError::DeliveryError(err)),
            }
        } else {
            Ok(())
        }
    }

    // pub fn send_response(&self, response: Response<T>) -> Result<bool, ()> {
    //     match response {
    //         Response::None => Ok(false),
    //         Response::Single(message) => self.send(message).map(|_| false),
    //         Response::Multiple(messages) => {
    //             for message in messages {
    //                 self.send(message)?;
    //             }
    //             Ok(false)
    //         }
    //         Response::Farewell => self.farewell().map(|_| true),
    //     }
    // }
}

/// The mailbox is used to receive messages from the penpal.
///
/// It can only receive messages of type T.
pub struct Mailbox<IncomingTransport: Read, T: for<'a> Deserialize<'a>> {
    incoming: IncomingTransport,
    _phantom: std::marker::PhantomData<T>,
}

impl<Incoming: Read, T: for<'a> Deserialize<'a>> Mailbox<Incoming, T> {
    /// Receive a message from the penpal.
    ///
    /// This function will block until a message is received. If the penpal has sent a farewell
    /// message, this function will return `None`.
    pub fn receive(&mut self) -> Result<Option<T>, ReceiveError> {
        let mut buffer = Vec::with_capacity(1024);
        receive(&mut self.incoming, &mut buffer)
    }

    /// Receive a message from the penpal using a given buffer as the receiving buffer.
    ///
    /// Same as `receive` but uses the given buffer to store the message temporarily, reducing
    /// allocations.
    pub fn receive_buf(&mut self, buffer: &mut Vec<u8>) -> Result<Option<T>, ReceiveError> {
        receive(&mut self.incoming, buffer)
    }
}

pub struct MailboxIntoIter<R: Read, T: for<'a> Deserialize<'a>> {
    mailbox: Mailbox<R, T>,
    buffer: Vec<u8>,
}

impl<R: Read, T: for<'a> Deserialize<'a>> IntoIterator for Mailbox<R, T> {
    type Item = Result<T, ReceiveError>;
    type IntoIter = MailboxIntoIter<R, T>;

    fn into_iter(self) -> Self::IntoIter {
        MailboxIntoIter {
            mailbox: self,
            buffer: Vec::with_capacity(1024),
        }
    }
}

impl<R: Read, T: for<'a> Deserialize<'a>> Iterator for MailboxIntoIter<R, T> {
    type Item = Result<T, ReceiveError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mailbox = &mut self.mailbox;
        let buffer = &mut self.buffer;
        match mailbox.receive_buf(buffer) {
            Ok(Some(message)) => Some(Ok(message)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }
}

/// A correspondance is a two-way communication with the penpal.
///
/// Receiving as well as sending messages is done asynchronously. Receiving messages is done using
/// a callback function. Sending messages is done using collection boxes.
pub struct Correspondence<O: Serialize, T: Send = ()> {
    collection_box: CollectionBox<O>,
    association: Arc<RwLock<T>>,
    result: Arc<(Mutex<Option<Result<(), CorrespondenceError>>>, Condvar)>,
}

impl<O: Serialize, T: Send> Correspondence<O, T> {
    /// Send a message to the penpal.
    pub fn send(&self, message: O) -> Result<(), CollectionBoxError> {
        self.collection_box.send(message)
    }

    /// Send a farewell message to the penpal.
    pub fn farewell(&self) -> Result<(), CollectionBoxError> {
        self.collection_box.farewell()
    }

    /// Get access to the collection box.
    pub fn collection_box(&self) -> &CollectionBox<O> {
        &self.collection_box
    }

    /// Get access to the association.
    ///
    /// Association is used to store a state that is shared between the individual invocations of
    /// the callback function and the correspondance.
    pub fn association(&self) -> &std::sync::RwLock<T> {
        &self.association
    }

    /// Wait for the correspondance to finish.
    ///
    /// This function will block until the correspondance has finished. If the correspondance has
    /// finished due to an error, this function will return the error.
    pub fn wait(&self) -> Result<(), CorrespondenceError> {
        let (result, cvar) = &*self.result;
        let mut result = result.lock().unwrap();

        loop {
            if let Some(result) = &*result {
                return result.clone();
            }
            result = cvar.wait(result).unwrap();
        }
    }
}

impl<IncomingTransport: Read, OutgoingTransport: Write + Send + 'static>
    Transport<IncomingTransport, OutgoingTransport>
{
    pub fn create_infrastructue<I: for<'a> Deserialize<'a>, O: Serialize + Send + 'static>(
        self,
    ) -> Result<(Mailbox<IncomingTransport, I>, CollectionBox<O>), std::io::Error> {
        let (post_office, collection_boxes) = std::sync::mpsc::channel();
        let Self { incoming, outgoing } = self;
        let result = Arc::new((Mutex::new(None), Condvar::new()));
        {
            let result = result.clone();
            std::thread::Builder::new()
                .name("post office".to_string())
                .spawn(move || {
                    CollectionBox::post_office(collection_boxes, outgoing, result);
                })?;
        }
        let mailbox = Mailbox {
            incoming,
            _phantom: std::marker::PhantomData,
        };
        let collection_box = CollectionBox {
            post_office,
            result,
        };
        Ok((mailbox, collection_box))
    }
}

#[derive(Error, Debug, Clone)]
#[error("correspondence has failed")]
pub enum CorrespondenceError {
    ReceiveError(#[from] ReceiveError),
    CollectionBoxError(#[from] CollectionBoxError),
}

impl<IncomingTransport: Read + Send + 'static, OutgoingTransport: Write + Send + 'static>
    Transport<IncomingTransport, OutgoingTransport>
{
    pub fn start_correspondence<
        I: for<'a> Deserialize<'a> + Send + 'static,
        O: Serialize + Send + 'static,
    >(
        self,
        mut f: impl FnMut(I, &CollectionBox<O>) -> Result<(), CorrespondenceError> + Send + 'static,
    ) -> Result<Correspondence<O, ()>, std::io::Error> {
        let (mailbox, collection_box) = self.create_infrastructue::<I, O>()?;
        let result = Arc::new((Mutex::new(None), Condvar::new()));

        {
            let collection_box = collection_box.clone();
            let result = result.clone();

            std::thread::Builder::new()
                .name("mailman".to_string())
                .spawn(move || {
                    for message in mailbox {
                        if let Err(err) = match message {
                            Ok(message) => f(message, &collection_box),
                            Err(err) => Err(CorrespondenceError::ReceiveError(err)),
                        } {
                            let (result, cvar) = &*result;
                            let mut result = result.lock().unwrap();
                            *result = Some(Err(err));
                            cvar.notify_all();
                            break;
                        }
                    }

                    let (result, cvar) = &*result;
                    let mut result = result.lock().unwrap();
                    match collection_box.farewell() {
                        Err(CollectionBoxError::DeliveryError(err)) => {
                            *result = Some(Err(CorrespondenceError::CollectionBoxError(
                                CollectionBoxError::DeliveryError(err),
                            )));
                        }
                        _ => {
                            *result = Some(Ok(()));
                        }
                    }
                    cvar.notify_all();
                })?;
        }

        Ok(Correspondence {
            collection_box,
            association: Arc::new(RwLock::new(())),
            result,
        })
    }

    pub fn start_correspondence_with_association<
        T: Send + Sync + 'static,
        I: for<'a> Deserialize<'a> + Send + 'static,
        O: Serialize + Send + 'static,
    >(
        self,
        association: T,
        mut f: impl FnMut(I, &RwLock<T>, &CollectionBox<O>) -> Result<(), CorrespondenceError>
            + Send
            + 'static,
    ) -> Result<Correspondence<O, T>, std::io::Error> {
        let (mailbox, collection_box) = self.create_infrastructue::<I, O>()?;
        let result = Arc::new((Mutex::new(None), Condvar::new()));
        let association = Arc::new(RwLock::new(association));

        {
            let collection_box = collection_box.clone();
            let result = result.clone();
            let association = association.clone();

            std::thread::Builder::new()
                .name("mailman".to_string())
                .spawn(move || {
                    for message in mailbox {
                        if let Err(err) = match message {
                            Ok(message) => f(message, &association, &collection_box),
                            Err(err) => Err(CorrespondenceError::ReceiveError(err)),
                        } {
                            let (result, cvar) = &*result;
                            let mut result = result.lock().unwrap();
                            *result = Some(Err(err));
                            cvar.notify_all();
                            break;
                        }
                    }
                    let (result, cvar) = &*result;
                    let mut result = result.lock().unwrap();
                    match collection_box.farewell() {
                        Err(CollectionBoxError::DeliveryError(err)) => {
                            *result = Some(Err(CorrespondenceError::CollectionBoxError(
                                CollectionBoxError::DeliveryError(err),
                            )));
                        }
                        _ => {
                            *result = Some(Ok(()));
                        }
                    }
                    cvar.notify_all();
                })?;
        }

        Ok(Correspondence {
            collection_box,
            association,
            result,
        })
    }
}
