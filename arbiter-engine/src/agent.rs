// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
// TODO: Notes ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
// When we start running an agent, we should have their messager start producing
// events that can be used by any and all behaviors the agent has that takes in
// messages as an event. Similarly, we should have agents start up any streams
// listeners that they need so those can also produce events. Those can then be
// piped into the behaviors that need them. Can perhaps make behaviors come from
// very specific events (e.g., specific contract events). This means each
// behavior should be a consumer and perhaps the agent itself is the producer
// (or at least relayer).
// This means we should give agents some way to "start streams" that they can
// then use to produce events.
// Behaviors definitely need to be able to reference the agent's client and
// messager so that they can send messages and interact with the blockchain.
// ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

//! The agent module contains the core agent abstraction for the Arbiter Engine.

use std::{fmt::Debug, pin::Pin, sync::Arc};

use arbiter_core::{data_collection::EventLogger, middleware::RevmMiddleware};
use ethers::contract::{EthLogDecode, Event};
use futures::stream::{Stream, StreamExt};
use futures_util::future::{join_all, JoinAll};
use serde::de::DeserializeOwned;
use tokio::task::JoinHandle;

use super::*;
use crate::{
    machine::{Behavior, Engine, State, StateMachine},
    messager::Messager,
    world::World,
};

// TODO: For the time being, these agents are just meant to be for arbiter
// instances. We can generalize later.

/// An agent is an entity capable of processing events and producing actions.
/// These are the core actors in simulations or in onchain systems.
/// Agents can be connected of other agents either as a dependent, or a
/// dependency.
///
/// # How it works
/// The [`Agent`] works by implementing the [`StateMachine`] trait. When the
/// [`World`] that owns the [`Agent`] is asked to enter into a new state, the
/// [`World`] will ask each [`Agent`] it owns to run that state transition by
/// calling [`StateMachine::run_state`]. All of the [`Agent`]s at once will then
/// will be able to be asked to block and wait to finish their state transition
/// by calling [`StateMachine::transition`]. Ultimately, the [`Agent`] will
/// transition through the following states:
/// 1. [`State::Uninitialized`]: The [`Agent`] has been created, but has not
///   been started.
/// 2. [`State::Syncing`]: The [`Agent`] is syncing with the world. This is
///  where the [`Agent`] can be brought up to date with the latest state of the
/// world. This could be used if the world was stopped and later restarted.
/// 3. [`State::Startup`]: The [`Agent`] is starting up. This is where the
/// [`Agent`] can be initialized and setup.
/// 4. [`State::Processing`]: The [`Agent`] is processing. This is where the
/// [`Agent`] can process events and produce actions. The [`State::Processing`]
/// stage may run for a long time before all [`Agent`]s are finished processing.
/// This is the main stage of the [`Agent`] that predominantly runs automation.
/// 5. [`State::Stopped`]: The [`Agent`] is stopped. This is where the [`Agent`]
/// can be stopped and state of the [`World`] and its [`Agent`]s can be
/// offloaded and saved.
pub struct Agent {
    /// Identifier for this agent.
    /// Used for routing messages.
    pub id: String,

    /// The status of the agent.
    pub state: State,

    /// The messager the agent uses to send and receive messages from other
    /// agents.
    pub messager: Messager,

    /// The client the agent uses to interact with the blockchain.
    pub client: Arc<RevmMiddleware>,

    /// The generalized event streamer for the agent that can stream a JSON
    /// `String`of any Ethereum event that can be decoded by behaviors.
    pub event_streamer: Option<EventLogger>,

    /// The engines/behaviors that the agent uses to sync, startup, and process
    /// events.
    behavior_engines: Option<Vec<Box<dyn StateMachine>>>,

    /// The tasks that represent the agent running a specific state transition.
    behavior_tasks: Option<JoinAll<JoinHandle<Box<dyn StateMachine>>>>,

    /// The pipeline for yielding events from the centralized event streamer
    /// (for both messages and Ethereum events) to agents.
    distributor: (
        async_broadcast::Sender<String>,
        async_broadcast::Receiver<String>,
    ),
}

impl Agent {
    /// Produces a new agent with the given identifier.
    pub(crate) fn connect(id: &str, world: &World) -> Self {
        let messager = world.messager.for_agent(id);
        let client = RevmMiddleware::new(&world.environment, Some(id)).unwrap();
        let distributor = async_broadcast::broadcast(512);
        Self {
            id: id.to_owned(),
            state: State::Uninitialized,
            messager,
            client,
            event_streamer: Some(EventLogger::builder()),
            behavior_engines: None,
            distributor,
            behavior_tasks: None,
        }
    }

    /// Adds an Ethereum event to the agent's event streamer.
    pub fn add_event<D: EthLogDecode + Debug + Serialize + 'static>(
        &mut self,
        event: Event<Arc<RevmMiddleware>, RevmMiddleware, D>,
    ) {
        self.event_streamer = Some(self.event_streamer.take().unwrap().add_stream(event));
    }

    /// Adds a behavior to the agent that it will run.
    pub fn add_behavior<E: DeserializeOwned + Send + Sync + Debug + 'static>(
        &mut self,
        behavior: impl Behavior<E> + 'static,
    ) {
        let event_receiver = self.distributor.0.new_receiver();

        let engine = Engine::new(behavior, event_receiver);
        if let Some(engines) = &mut self.behavior_engines {
            engines.push(Box::new(engine));
        } else {
            self.behavior_engines = Some(vec![Box::new(engine)]);
        }
    }

    // TODO: This is unused for now, but we will use it in the future for the event
    // pipelining.
    #[allow(unused)]
    pub(crate) fn start_event_stream(&mut self) -> Pin<Box<dyn Stream<Item = String> + Send + '_>> {
        let event_stream = self.event_streamer.take().unwrap().stream();
        let message_stream = self
            .messager
            .stream()
            .map(|msg| serde_json::to_string(&msg).unwrap_or_else(|e| e.to_string()));

        Box::pin(futures::stream::select(event_stream, message_stream))
    }
}

#[async_trait::async_trait]
impl StateMachine for Agent {
    fn run_state(&mut self, state: State) {
        match state {
            State::Uninitialized => {
                unimplemented!("This never gets called.")
            }
            State::Syncing => {
                self.state = state;
                trace!("Agent is syncing.");
                let mut behavior_engines = self.behavior_engines.take().unwrap();
                for engine in behavior_engines.iter_mut() {
                    engine.run_state(state);
                }
                self.behavior_tasks =
                    Some(join_all(behavior_engines.into_iter().map(|mut engine| {
                        tokio::spawn(async move {
                            engine.transition().await;
                            engine
                        })
                    })));
            }
            State::Startup => {
                trace!("Agent is starting up.");
                self.state = state;
                let mut behavior_engines = self.behavior_engines.take().unwrap();
                for engine in behavior_engines.iter_mut() {
                    engine.run_state(state);
                }
                self.behavior_tasks =
                    Some(join_all(behavior_engines.into_iter().map(|mut engine| {
                        tokio::spawn(async move {
                            engine.transition().await;
                            engine
                        })
                    })));
            }
            State::Processing => {
                trace!("Agent is processing.");
                self.state = state;
                let mut behavior_engines = self.behavior_engines.take().unwrap();
                for engine in behavior_engines.iter_mut() {
                    engine.run_state(state);
                }
                self.behavior_tasks =
                    Some(join_all(behavior_engines.into_iter().map(|mut engine| {
                        tokio::spawn(async move {
                            engine.transition().await;
                            engine
                        })
                    })));
            }
            State::Stopped => {
                todo!()
            }
        }
    }

    async fn transition(&mut self) {
        self.behavior_engines = Some(
            self.behavior_tasks
                .take()
                .unwrap()
                .await
                .into_iter()
                .map(|res| res.unwrap())
                .collect::<Vec<_>>(),
        );
    }
}

#[cfg(test)]
mod tests {
    use arbiter_bindings::bindings::arbiter_token::ArbiterToken;
    use ethers::types::U256;

    use super::*;
    use crate::messager::Message;
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streaming() {
        std::env::set_var("RUST_LOG", "trace");
        tracing_subscriber::fmt::init();

        let mut world = World::new("world");
        let messager = world.messager.clone();
        println!(
            "Receiver count: {:?}",
            messager.broadcast_sender.receiver_count()
        );

        let agent = world.create_agent("agent");

        let arb = ArbiterToken::deploy(
            agent.client.clone(),
            ("ArbiterToken".to_string(), "ARB".to_string(), 18u8),
        )
        .unwrap()
        .send()
        .await
        .unwrap();

        agent.add_event(arb.events());
        let address = agent.client.address();
        let mut streamer = agent.start_event_stream();

        for _ in 0..5 {
            messager
                .send(Message {
                    from: "me".to_string(),
                    to: messager::To::All,
                    data: "hello".to_string(),
                })
                .await;
            arb.approve(address, U256::from(1))
                .send()
                .await
                .unwrap()
                .await
                .unwrap();
        }

        while let Some(msg) = streamer.next().await {
            println!("Printing message in test: {:?}", msg);
        }
    }
}
