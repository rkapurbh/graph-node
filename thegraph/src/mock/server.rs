use tokio;
use futures::prelude::*;
use futures::future;
use futures::sync::mpsc::{channel, Receiver, Sender};
use futures::sync::oneshot;
use hyper;
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use hyper::service::Service;
use prelude::*;
use common::query::{Query, QueryResult};
use common::schema::SchemaProviderEvent;
use common::server::GraphQLServerError;
use common::store::StoreEvent;
use common::util::stream::StreamError;

/// An asynchronous response to a GraphQL request.
type GraphQLServiceResponse = Box<Future<Item = Response<Body>, Error = GraphQLServerError> + Send>;

/// Future for HTTP responses to GraphQL query requests.
struct GraphQLResponse {
    result: Result<QueryResult, GraphQLServerError>,
}

impl GraphQLResponse {
    /// Creates a new GraphQLResponse future based on the result generated by
    /// running a query.
    pub fn new(result: Result<QueryResult, GraphQLServerError>) -> Self {
        GraphQLResponse { result }
    }
}

impl Future for GraphQLResponse {
    type Item = Response<Body>;
    type Error = GraphQLServerError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.result {
            // Query was successful -> return a mock response
            Ok(ref result) => Ok(Async::Ready({
                let data = format!("{:?}", result);
                Response::builder()
                    .status(200)
                    .body(Body::from(data))
                    .unwrap()
            })),

            // Query caused an error -> return a mock error response
            Err(ref e) => {
                let err = format!("{}", e);
                Ok(Async::Ready(
                    Response::builder()
                        .status(500)
                        .body(Body::from(err))
                        .unwrap(),
                ))
            }
        }
    }
}

/// A Hyper Service that serves GraphQL over a POST / endpoint.
#[derive(Debug)]
struct GraphQLService {
    query_sink: Sender<Query<Request<Body>>>,
}

impl GraphQLService {
    /// Creates a new GraphQL service.
    pub fn new(query_sink: Sender<Query<Request<Body>>>) -> Self {
        GraphQLService { query_sink }
    }

    /// Handles GraphQL queries received via POST /.
    fn handle_graphql_query(&self, req: Request<Body>) -> GraphQLServiceResponse {
        println!("Handle GraphQL query: {:?}", req);

        // Create a one-shot channel to allow another part of the system
        // to notify the service when the query has completed
        let (sender, receiver) = oneshot::channel();

        // Send the query to whoever else will run it
        tokio::spawn(
            self.query_sink
                .clone()
                .send(Query {
                    request: req,
                    result_sender: sender,
                })
                .map_err(|e| panic!("Failed to feed query into the system: {}", e))
                .and_then(|_| Ok(())),
        );

        // Create a response future that translates the query result back to an
        // HTTP response as soon as the query result is satisfied
        Box::new(
            receiver
                .map_err(|e| GraphQLServerError::from(e))
                .then(|result| GraphQLResponse::new(result)),
        )
    }

    /// Handles 404s.
    fn handle_not_found(&self, _req: Request<Body>) -> GraphQLServiceResponse {
        Box::new(future::ok(
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Not found"))
                .unwrap(),
        ))
    }
}

impl Service for GraphQLService {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = GraphQLServerError;
    type Future = GraphQLServiceResponse;

    fn call(&mut self, req: Request<Self::ReqBody>) -> Self::Future {
        match (req.method(), req.uri().path()) {
            // POST / receives GraphQL queries
            (&Method::POST, "/") => self.handle_graphql_query(req),

            // Everything else results in a 404
            _ => self.handle_not_found(req),
        }
    }
}

/// A mock [GraphQLServer](../common/server/trait.GraphQLServer.html) based on Hyper.
pub struct MockGraphQLServer {
    query_sink: Option<Sender<Query<Request<Body>>>>,
    schema_provider_event_sink: Sender<SchemaProviderEvent>,
    store_event_sink: Sender<StoreEvent>,
}

impl MockGraphQLServer {
    /// Creates a new mock [GraphQLServer](../common/server/trait.GraphQLServer.html).
    pub fn new() -> Self {
        // Create channels for handling incoming events from the schema provider and the store
        let (store_sink, store_stream) = channel(100);
        let (schema_provider_sink, schema_provider_stream) = channel(100);

        // Create a new mock GraphQL server
        let mut server = MockGraphQLServer {
            query_sink: None,
            schema_provider_event_sink: schema_provider_sink,
            store_event_sink: store_sink,
        };

        // Spawn tasks to handle incoming events from the schema provider and store
        server.handle_schema_provider_events(schema_provider_stream);
        server.handle_store_events(store_stream);

        // Return the new server
        server
    }

    /// Handle incoming events from the schema provider
    fn handle_schema_provider_events(&mut self, stream: Receiver<SchemaProviderEvent>) {
        tokio::spawn(stream.for_each(|event| {
            println!(
                "GraphQL server: Received schema provider event: {:?}",
                event
            );
            Ok(())
        }));
    }

    // Handle incoming events from the store
    fn handle_store_events(&mut self, stream: Receiver<StoreEvent>) {
        tokio::spawn(stream.for_each(|event| {
            println!("GraphQL server: Received store event: {:?}", event);
            Ok(())
        }));
    }
}

impl GraphQLServer<Request<Body>> for MockGraphQLServer {
    fn schema_provider_event_sink(&mut self) -> Sender<SchemaProviderEvent> {
        self.schema_provider_event_sink.clone()
    }

    fn store_event_sink(&mut self) -> Sender<StoreEvent> {
        self.store_event_sink.clone()
    }

    fn query_stream(&mut self) -> Result<Receiver<Query<Request<Body>>>, StreamError> {
        // If possible, create a new channel for streaming incoming queries
        match self.query_sink {
            Some(_) => Err(StreamError::AlreadyCreated),
            None => {
                let (sink, stream) = channel(100);
                self.query_sink = Some(sink);
                Ok(stream)
            }
        }
    }

    fn serve(&mut self) -> Result<Box<Future<Item = (), Error = ()> + Send>, GraphQLServerError> {
        // We will listen on port 8000
        let addr = "0.0.0.0:8000".parse().unwrap();

        // Only launch the GraphQL server if there is a component that will handle incoming queries
        match self.query_sink {
            Some(ref query_sink) => {
                // On every incoming request, launch a new GraphQL service that writes
                // incoming queries to the query sink.
                let query_sink = query_sink.clone();
                let new_service = move || {
                    let service = GraphQLService::new(query_sink.clone());
                    future::ok::<GraphQLService, hyper::Error>(service)
                };

                // Create a task to run the server and handle HTTP requests
                let task = Server::bind(&addr)
                    .serve(new_service)
                    .map_err(|e| eprintln!("GraphQL server error: {}", e));

                Ok(Box::new(task))
            }
            None => Err(GraphQLServerError::InternalError(
                "No component set up to handle incoming queries",
            )),
        }
    }
}