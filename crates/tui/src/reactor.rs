use std::io;
use std::thread;

use crossbeam_channel::{Receiver, Sender as ChannelSender};
use crossterm::event::{self, Event as TerminalEvent};

enum Message<T> {
    Terminal(io::Result<TerminalEvent>),
    Work(T),
}

pub(crate) enum Event<T> {
    Terminal(TerminalEvent),
    Work(T),
}

pub(crate) struct Sender<T>(ChannelSender<Message<T>>);

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Sender<T> {
    pub(crate) fn send(&self, work: T) -> bool {
        self.0.send(Message::Work(work)).is_ok()
    }
}

pub(crate) struct Reactor<T> {
    receiver: Receiver<Message<T>>,
}

impl<T: Send + 'static> Reactor<T> {
    pub(crate) fn new() -> (Self, Sender<T>) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let terminal = sender.clone();
        thread::spawn(move || {
            loop {
                let event = event::read();
                let failed = event.is_err();
                if terminal.send(Message::Terminal(event)).is_err() || failed {
                    break;
                }
            }
        });
        (Self { receiver }, Sender(sender))
    }

    pub(crate) fn wait(&mut self) -> io::Result<Event<T>> {
        match self.receiver.recv() {
            Ok(Message::Terminal(event)) => event.map(Event::Terminal),
            Ok(Message::Work(work)) => Ok(Event::Work(work)),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "event reactor disconnected",
            )),
        }
    }
}
