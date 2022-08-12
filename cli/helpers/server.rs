use crate::helpers::Ledger;
use snarkvm::prelude::{Field, GraphKey, Network, RecordsFilter, Transaction, ViewKey};

use anyhow::Result;
use core::marker::PhantomData;
use indexmap::IndexMap;
use std::sync::Arc;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use warp::{http::StatusCode, reject, reply, Filter, Rejection, Reply};

/// An enum of error handlers for the server.
#[derive(Debug)]
enum ServerError {
    Request(String),
}

impl reject::Reject for ServerError {}

/// A trait to unwrap a `Result` or `Reject`.
pub trait OrReject<T> {
    /// Returns the result if it is successful, otherwise returns a rejection.
    fn or_reject(self) -> Result<T, Rejection>;
}

impl<T> OrReject<T> for anyhow::Result<T> {
    /// Returns the result if it is successful, otherwise returns a rejection.
    fn or_reject(self) -> Result<T, Rejection> {
        Ok(self.map_err(|e| reject::custom(ServerError::Request(e.to_string())))?)
    }
}

/// A middleware to include the given item in the handler.
fn with<T: Clone + Send>(item: T) -> impl Filter<Extract = (T,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || item.clone())
}

/// Shorthand for the parent half of the `Ledger` message channel.
pub type LedgerSender<N> = mpsc::Sender<LedgerRequest<N>>;
/// Shorthand for the child half of the `Ledger` message channel.
pub type LedgerReceiver<N> = mpsc::Receiver<LedgerRequest<N>>;

/// An enum of requests that the `Ledger` struct processes.
#[derive(Debug)]
pub enum LedgerRequest<N: Network> {
    TransactionBroadcast(Transaction<N>),
}

/// A server for the ledger.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Server<N: Network> {
    /// The runtime.
    runtime: tokio::runtime::Runtime,
    /// The ledger sender.
    ledger_sender: LedgerSender<N>,
    /// The server handles.
    handles: Vec<JoinHandle<()>>,
    /// PhantomData.
    _phantom: PhantomData<N>,
}

impl<N: Network> Server<N> {
    /// Initializes a new instance of the server.
    pub fn start(ledger: Arc<Ledger<N>>) -> Result<Self> {
        // Initialize a channel to send requests to the ledger.
        let (ledger_sender, ledger_receiver) = mpsc::channel(64);

        // GET /testnet3/latest/height
        let latest_height = warp::get()
            .and(warp::path!("testnet3" / "latest" / "height"))
            .and(with(ledger.clone()))
            .and_then(Self::latest_height);

        // GET /testnet3/latest/hash
        let latest_hash = warp::get()
            .and(warp::path!("testnet3" / "latest" / "hash"))
            .and(with(ledger.clone()))
            .and_then(Self::latest_hash);

        // GET /testnet3/latest/block
        let latest_block = warp::get()
            .and(warp::path!("testnet3" / "latest" / "block"))
            .and(with(ledger.clone()))
            .and_then(Self::latest_block);

        // GET /testnet3/block/{height}
        let get_block = warp::get()
            .and(warp::path!("testnet3" / "block" / u32))
            .and(with(ledger.clone()))
            .and_then(Self::get_block);

        // GET /testnet3/statePath/{commitment}
        let state_path = warp::get()
            .and(warp::path!("testnet3" / "statePath"))
            .and(warp::body::content_length_limit(128))
            .and(warp::body::json())
            .and(with(ledger.clone()))
            .and_then(Self::state_path);

        // GET /testnet3/records/all
        let records_all = warp::get()
            .and(warp::path!("testnet3" / "records" / "all"))
            .and(warp::body::content_length_limit(256))
            .and(warp::body::json())
            .and(with(ledger.clone()))
            .and_then(Self::records_all);

        // GET /testnet3/records/spent
        let records_spent = warp::get()
            .and(warp::path!("testnet3" / "records" / "spent"))
            .and(warp::body::content_length_limit(256))
            .and(warp::body::json())
            .and(with(ledger.clone()))
            .and_then(Self::records_spent);

        // GET /testnet3/records/unspent
        let records_unspent = warp::get()
            .and(warp::path!("testnet3" / "records" / "unspent"))
            .and(warp::body::content_length_limit(128))
            .and(warp::body::json())
            .and(with(ledger.clone()))
            .and_then(Self::records_unspent);

        // POST /testnet3/transaction/broadcast
        let transaction_broadcast = warp::post()
            .and(warp::path!("testnet3" / "transaction" / "broadcast"))
            .and(warp::body::content_length_limit(10 * 1024 * 1024))
            .and(warp::body::json())
            .and(with(ledger_sender.clone()))
            .and_then(Self::transaction_broadcast);

        // Initialize a runtime.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(8 * 1024 * 1024)
            .build()?;

        // Initialize a vector for the server handles.
        let mut handles = Vec::new();

        // Spawn the ledger handler.
        handles.push(runtime.block_on(Self::start_handler(ledger, ledger_receiver)));

        // Use a oneshot channel to ensure that the warp task has started.
        let (tx_warp_ready, rx_warp_ready) = oneshot::channel::<()>();

        // Spawn the server.
        handles.push(tokio::spawn(async move {
            // Prepare the list of routes.
            let routes = latest_height
                .or(latest_hash)
                .or(latest_block)
                .or(get_block)
                .or(state_path)
                .or(records_all)
                .or(records_spent)
                .or(records_unspent)
                .or(transaction_broadcast);

            // Notify that the warp server task is ready.
            tx_warp_ready.send(()).unwrap();

            // Start the server.
            println!("\n🌐 Server is running at http://0.0.0.0:4180");
            warp::serve(routes).run(([0, 0, 0, 0], 4180)).await;
        }));

        // Wait until the readiness notification is received.
        runtime.block_on(rx_warp_ready).unwrap();

        Ok(Self {
            runtime,
            ledger_sender,
            handles,
            _phantom: PhantomData,
        })
    }

    /// Initializes a ledger handler.
    async fn start_handler(ledger: Arc<Ledger<N>>, mut ledger_receiver: LedgerReceiver<N>) -> JoinHandle<()> {
        // Use a oneshot channel to ensure that the handler task has started.
        let (tx_handler_ready, rx_handler_ready) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            tx_handler_ready.send(()).unwrap();

            while let Some(request) = ledger_receiver.recv().await {
                match request {
                    LedgerRequest::TransactionBroadcast(transaction) => {
                        if let Err(error) = ledger.add_to_memory_pool(transaction) {
                            eprintln!("{error}")
                        }
                    }
                };
            }
        });

        // Wait until the readiness notification is received.
        rx_handler_ready.await.unwrap();

        handle
    }
}

impl<N: Network> Server<N> {
    /// Returns the latest block height.
    async fn latest_height(ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        Ok(reply::json(&ledger.ledger.read().latest_height()))
    }

    /// Returns the latest block hash.
    async fn latest_hash(ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        Ok(reply::json(&ledger.ledger.read().latest_hash()))
    }

    /// Returns the latest block.
    async fn latest_block(ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        Ok(reply::json(&ledger.ledger.read().latest_block().or_reject()?))
    }

    /// Returns the block for the given block height.
    async fn get_block(height: u32, ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        Ok(reply::json(&ledger.ledger.read().get_block(height).or_reject()?))
    }

    /// Returns the state path for the given commitment.
    async fn state_path(commitment: Field<N>, ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        Ok(reply::json(
            &ledger.ledger.read().to_state_path(&commitment).or_reject()?,
        ))
    }

    /// Returns all of the records for the given view key.
    async fn records_all(view_key: ViewKey<N>, ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        // Fetch the records using the view key.
        let records: IndexMap<_, _> = ledger
            .ledger
            .read()
            .find_records(&view_key, RecordsFilter::All)
            .collect();
        // Return the records.
        Ok(reply::with_status(reply::json(&records), StatusCode::OK))
    }

    /// Returns the spent records for the given view key.
    async fn records_spent(body: IndexMap<String, String>, ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        // Parse the body.
        let view_key: ViewKey<N> = body["view_key"].parse().or_reject()?;
        let graph_key: GraphKey<N> = body["graph_key"].parse().or_reject()?;
        // Fetch the records using the view key.
        let records = ledger
            .ledger
            .read()
            .find_records(&view_key, RecordsFilter::Spent(graph_key))
            .collect::<IndexMap<_, _>>();
        // Return the records.
        Ok(reply::with_status(reply::json(&records), StatusCode::OK))
    }

    /// Returns the unspent records for the given view key.
    async fn records_unspent(body: IndexMap<String, String>, ledger: Arc<Ledger<N>>) -> Result<impl Reply, Rejection> {
        // Parse the body.
        let view_key: ViewKey<N> = body["view_key"].parse().or_reject()?;
        let graph_key: GraphKey<N> = body["graph_key"].parse().or_reject()?;
        // Fetch the records using the view key.
        let records = ledger
            .ledger
            .read()
            .find_records(&view_key, RecordsFilter::Unspent(graph_key))
            .collect::<IndexMap<_, _>>();
        // Return the records.
        Ok(reply::with_status(reply::json(&records), StatusCode::OK))
    }

    /// Broadcasts the transaction to the ledger.
    async fn transaction_broadcast(
        transaction: Transaction<N>,
        ledger_sender: LedgerSender<N>,
    ) -> Result<impl Reply, Rejection> {
        // Send the transaction to the ledger.
        match ledger_sender
            .send(LedgerRequest::TransactionBroadcast(transaction))
            .await
        {
            Ok(()) => Ok("OK"),
            Err(error) => Err(reject::custom(ServerError::Request(format!("{error}")))),
        }
    }
}
