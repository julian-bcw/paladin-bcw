//! Provides a [`Sink`] implementation generic over any [`QueueHandle`].
//!
//! This module introduces a `QueueSink` that abstracts the intricacies of a
//! queue system, allowing users to interact with it as a simple [`Sink`].
//! This allows decoupling downstream components from queues such that they can
//! simply expect standard [`Sink`] behavior.
//!
//! # Examples
//!
//! ```no_run
//! # use paladin::queue::{Connection, QueueOptions, amqp::{AMQPConnection, AMQPConnectionOptions}};
//! # use paladin::queue::sink::QueueSink;
//! # use anyhow::Result;
//! use serde::{Serialize, Deserialize};
//! use futures::{Sink, SinkExt};
//!
//! #[derive(Serialize, Deserialize)]
//! struct MyStruct {
//!     field: String,
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<()> {
//! let conn = AMQPConnection::new(AMQPConnectionOptions {
//!     uri: "amqp://localhost:5672",
//!     qos: Some(1),
//!     serializer: Default::default(),
//! }).await?;
//! let queue = conn.declare_queue("my_queue", QueueOptions::default()).await?;
//! let mut sink = QueueSink::new(queue);
//! sink.send(MyStruct { field: "hello world".to_string() }).await?;
//!
//! Ok(())
//! # }
//! ```
//!
//! # Design Notes
//!
//! - The `QueueSink` struct holds a phantom data marker for type safety without
//!   runtime overhead.
//! - The `From` trait is implemented for `QueueSink`, allowing for easy
//!   conversion from a `QueueHandle`.
//! - The sink's readiness is always set to ready, as it expects a `QueueHandle`
//!   as a parameter, which already has an established connection.
//! - The `poll_flush` method ensures that all spawned tasks are completed
//!   before the sink is considered flushed.
//! - The `start_send` method spawns a new task for each item, ensuring
//!   asynchronous processing.

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use anyhow::Result;
use futures::{future::BoxFuture, ready, stream::FuturesOrdered, FutureExt, Sink, StreamExt};
use pin_project::pin_project;

use crate::{queue::QueueHandle, serializer::Serializable};

/// A generic [`Sink`] implementation for [`QueueHandle`].
/// Abstracts away a Queue dependency from the caller such they may simply
/// require a [`Sink`].
#[pin_project]
pub struct QueueSink<'a, Data, Handle> {
    _phantom: std::marker::PhantomData<Data>,
    queue_handle: Handle,
    send_futures: FuturesOrdered<BoxFuture<'a, Result<()>>>,
}

impl<'a, Data, Handle> QueueSink<'a, Data, Handle> {
    /// Create a new [`QueueSink`] instance from a [`QueueHandle`].
    ///
    ///
    /// ```no_run
    /// # use paladin::queue::{Connection, QueueOptions, amqp::{AMQPConnection, AMQPConnectionOptions}};
    /// # use paladin::queue::sink::QueueSink;
    /// # use anyhow::Result;
    /// use serde::{Serialize, Deserialize};
    /// use futures::{Sink, SinkExt};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct MyStruct {
    ///     field: String,
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<()> {
    /// let conn = AMQPConnection::new(AMQPConnectionOptions {
    ///     uri: "amqp://localhost:5672",
    ///     qos: Some(1),
    ///     serializer: Default::default(),
    /// }).await?;
    /// let queue = conn.declare_queue("my_queue", QueueOptions::default()).await?;
    /// let mut sink = QueueSink::new(queue);
    /// sink.send(MyStruct { field: "hello world".to_string() }).await?;
    ///
    /// Ok(())
    /// # }
    pub fn new(queue_handle: Handle) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            queue_handle,
            send_futures: FuturesOrdered::new(),
        }
    }
}

impl<'a, Data, Handle> Sink<Data> for QueueSink<'a, Data, Handle>
where
    Data: Serializable + 'a,
    Handle: QueueHandle + Send + Sync + 'a,
{
    type Error = anyhow::Error;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        // Flush the sink.
        ready!(self.poll_flush(cx)?);

        Poll::Ready(Ok(()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        loop {
            match self.send_futures.poll_next_unpin(cx) {
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(Some(Ok(_))) => continue,
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Data) -> Result<()> {
        let this = self.get_mut();
        let queue_handle = this.queue_handle.clone();

        let fut = async move { queue_handle.publish(&item).await };
        this.send_futures.push_back(fut.boxed());

        Ok(())
    }
}
