//! The messager module contains the core messager layer for the Arbiter Engine.

use std::pin::Pin;

use async_broadcast::{broadcast, Receiver as BroadcastReceiver, Sender as BroadcastSender};
use futures_util::Stream;

use super::*;

/// A message that can be sent between agents.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Message {
    /// The sender of the message.
    pub from: String,

    /// The recipient of the message.
    pub to: To,

    /// The data of the message.
    /// This can be a struct serialized into JSON.
    pub data: String,
}

/// The recipient of the message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum To {
    /// Send the message to all agents who are listening for broadcasts.
    All,

    /// Send the message to a specific agent.
    Agent(String),
}

/// A messager that can be used to send messages between agents.
#[derive(Clone, Debug)]
pub struct Messager {
    /// The identifier of the entity that is using the messager.
    pub id: Option<String>,

    pub(crate) broadcast_sender: BroadcastSender<Message>,

    broadcast_receiver: BroadcastReceiver<Message>,
}

impl Messager {
    // TODO: Allow for modulating the capacity of the messager.
    // TODO: It might be nice to have some kind of messaging header so that we can
    // pipe messages to agents and pipe messages across worlds.
    /// Creates a new messager with the given capacity.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (broadcast_sender, broadcast_receiver) = broadcast(512);
        Self {
            broadcast_sender,
            broadcast_receiver,
            id: None,
        }
    }

    // TODO: Okay if we do something kinda like this, then agents don't even need to
    // filter the `to` field or set the `from` field. Let's give this a shot!
    pub(crate) fn for_agent(&self, id: &str) -> Self {
        Self {
            broadcast_sender: self.broadcast_sender.clone(),
            broadcast_receiver: self.broadcast_receiver.clone(),
            id: Some(id.to_owned()),
        }
    }

    /// Returns a stream of messages that are either sent to [`To::All`] or to
    /// the agent via [`To::Agent(id)`].
    pub fn stream(&self) -> Pin<Box<dyn Stream<Item = Message> + Send + '_>> {
        let mut receiver = self.broadcast_receiver.clone();
        let stream = async_stream::stream! {
            while let Ok(message) = receiver.recv().await {
                match &message.to {
                    To::All => {
                        yield message;
                    }
                    To::Agent(id) => {
                        if let Some(self_id) = &self.id {
                            if id == self_id {
                                yield message;
                            }
                        }
                    }
                }
            }
        };
        Box::pin(stream)
    }

    /// Sends a message to the messager.
    pub async fn send(&self, message: Message) {
        trace!("Sending message via messager.");
        self.broadcast_sender.broadcast(message).await.unwrap();
    }
}
