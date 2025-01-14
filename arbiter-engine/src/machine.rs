//! The [`StateMachine`] trait, [`Behavior`] trait, and the [`Engine`] that runs
//! [`Behavior`]s.

use std::fmt::Debug;

use async_broadcast::Receiver;
use serde::de::DeserializeOwned;
use tokio::task::JoinHandle;

use super::*;

/// The state used by any entity implementing [`StateMachine`].
#[derive(Debug, Copy, Clone)]
pub enum State {
    /// The entity is not yet running any process.
    /// This is the state adopted by the entity when it is first created.
    Uninitialized,

    /// The entity is syncing with the world.
    /// This can be used to bring the entity back up to date with the latest
    /// state of the world. This could be used if the world was stopped and
    /// later restarted.
    Syncing,

    /// The entity is starting up.
    /// This is where the entity can engage in its specific start up activities
    /// that it can do given the current state of the world.
    /// These are usually quick one-shot activities that are not repeated.
    Startup,

    /// The entity is processing.
    /// This is where the entity can engage in its specific processing
    /// of events that can lead to actions being taken.
    Processing,

    /// The entity is stopped.
    /// This is where state can be offloaded and saved if need be.
    Stopped,
}

// NOTE: `async_trait::async_trait` is used throughout to make the trait object
// safe even though rust >=1.75 has async trait stabilized

/// The [`Behavior`] trait is the lowest level functionality that will be used
/// by a [`StateMachine`]. This constitutes what each state transition will do.
#[async_trait::async_trait]
pub trait Behavior<E>: Send + Sync + 'static {
    /// Used to bring the agent back up to date with the latest state of the
    /// world. This could be used if the world was stopped and later restarted.
    async fn sync(&mut self);

    /// Used to start the agent.
    /// This is where the agent can engage in its specific start up activities
    /// that it can do given the current state of the world.
    async fn startup(&mut self);

    /// Used to process events.
    /// This is where the agent can engage in its specific processing
    /// of events that can lead to actions being taken.
    async fn process(&mut self, event: E);
}

#[async_trait::async_trait]
pub(crate) trait StateMachine: Send + Sync + 'static {
    fn run_state(&mut self, state: State);
    async fn transition(&mut self);
}

/// The idea of the [`Engine`] is that it drives the [`Behavior`] of a
/// [`StateMachine`]-based entity (like an [`agent::Agent`]).
/// The [`Engine`] specifically wraps a [`Behavior`] and a [`Receiver`] of
/// events into a cohesive unit that can listen to events and pass them onto the
/// processor stage. Since the [`Engine`] is also a [`StateMachine`], its
/// generics can be collapsed into a `dyn` trait object so that, for example,
/// [`agent::Agent`]s can own multiple [`Behavior`]s with different event `<E>`
/// types.
pub struct Engine<B, E>
where
    B: Behavior<E>,
{
    /// The behavior the [`Engine`] runs.
    pub behavior: Option<B>,

    /// When the state of the [`Engine`] is modified, the tasks will be run (and
    /// therefore `move`d) concurrently and stored here.
    behavior_task: Option<JoinHandle<B>>,

    /// The receiver of events that the [`Engine`] will process.
    /// The [`State::Processing`] stage will attempt a decode of the [`String`]s
    /// into the event type `<E>`.
    event_receiver: Receiver<String>,

    phantom: std::marker::PhantomData<E>,
}

impl<B, E> Engine<B, E>
where
    B: Behavior<E>,
    E: DeserializeOwned + Send + Sync + 'static,
{
    /// Creates a new [`Engine`] with the given [`Behavior`] and [`Receiver`].
    pub(crate) fn new(behavior: B, event_receiver: Receiver<String>) -> Self {
        Self {
            behavior: Some(behavior),
            behavior_task: None,
            event_receiver,
            phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<B, E> StateMachine for Engine<B, E>
where
    B: Behavior<E>,
    E: DeserializeOwned + Send + Sync + Debug + 'static,
{
    fn run_state(&mut self, state: State) {
        match state {
            State::Uninitialized => {
                unimplemented!("This never gets called.")
            }
            State::Syncing => {
                trace!("Behavior is syncing.");
                let mut behavior = self.behavior.take().unwrap();
                self.behavior_task = Some(tokio::spawn(async move {
                    behavior.sync().await;
                    behavior
                }));
            }
            State::Startup => {
                trace!("Behavior is starting up.");
                let mut behavior = self.behavior.take().unwrap();
                self.behavior_task = Some(tokio::spawn(async move {
                    behavior.startup().await;
                    behavior
                }));
            }
            State::Processing => {
                trace!("Behavior is processing.");
                let mut behavior = self.behavior.take().unwrap();
                let mut receiver = self.event_receiver.clone(); // TODO Could use Option::take() if we don't want to clone.
                self.behavior_task = Some(tokio::spawn(async move {
                    while let Ok(event) = receiver.recv().await {
                        trace!("Behavior has gotten event: {:?}", event);
                        let decoding_result = serde_json::from_str::<E>(&event);
                        match decoding_result {
                            Ok(event) => behavior.process(event).await,
                            Err(e) => {
                                tracing::error!("Error decoding event: {:?}", e);
                                continue;
                            }
                        }
                    }
                    behavior
                }));
            }
            State::Stopped => {
                todo!()
            }
        }
    }

    /// Take the task and wait for it to finish. Then take the [`Behavior`] and
    /// put it back into the engine.
    async fn transition(&mut self) {
        let behavior_task = self.behavior_task.take().unwrap();
        let behavior = behavior_task.await.unwrap();
        self.behavior = Some(behavior);
    }
}
