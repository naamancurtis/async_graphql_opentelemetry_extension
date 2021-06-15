//! # Async GraphQL Telemetry Extension
//!
//! The `Extensions` trait in [async-graphql](https://github.com/async-graphql/async-graphql) essentially mimics traditional
//! middleware in HTTP servers (although arguably more powerful due to the
//! ability to hook into various stages of the query resolution). This extension is an
//! attempt at adding in some of the Open Telemetry integrations in order
//! to handle metric and trace creation through this API, as opposed to manually
//! having to instrument every query.
//!
//! It is essentially a straight copy and paste from the [ApolloTracing](https://docs.rs/async-graphql/latest/async_graphql/extensions/struct.ApolloTracing.html) & [Tracing](https://docs.rs/async-graphql/latest/async_graphql/extensions/struct.Tracing.html) Extensions from
//! the core library, just modified to enable metric creation and a slightly
//! different span generation pattern.
//!
//! ## Features
//!
//! This extension includes
//! - Tracing (via [tracing](https://github.com/tokio-rs/tracing))
//! - High Level Metrics (via [OpenTelemetry](https://github.com/open-telemetry/opentelemetry-rust/tree/main/opentelemetry))
//!
//! ## Reason for combining the extensions
//!
//! The primary reason for combining these extensions is to minimise the amount of data required to
//! be stored per request. As an example, in order to generate the high level metrics, the Apollo Tracing data can
//! be used. So combining all 3 made the most sense for minimising the space and
//! computation done while processing requests.
//!
//! ## License
//!
//! Anything found within here falls under the same licenses as the main
//! repository, which can be found here <https://github.com/async-graphql/async-graphql>
//!
//! MIT or Apache version 2.0

use opentelemetry::metrics::{Counter, ValueRecorder};
use opentelemetry::{global, Key, Unit};

use lazy_static::lazy_static;

use futures_util::stream::BoxStream;
use futures_util::TryFutureExt;
use tokio::time::Instant;
use tracing::{span, Level};
use tracing_futures::Instrument;

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextExecute, NextParseQuery, NextRequest,
    NextResolve, NextSubscribe, NextValidation, ResolveInfo,
};
use async_graphql::parser::types::ExecutableDocument;
use async_graphql::{Response, ServerError, ServerResult, ValidationResult, Value, Variables};

use std::sync::Arc;

lazy_static! {
    static ref REQUESTS: Counter<u64> = {
        let meter = global::meter(NAME);
        let counter = meter
            .u64_counter("graphql_requests")
            .with_description("total number of HTTP requests sent to the graphQL server")
            .init();
        counter
    };
    static ref SUBSCRIPTIONS: Counter<u64> = {
        let meter = global::meter(NAME);
        let counter = meter
            .u64_counter("graphql_subscriptions")
            .with_description("total number of subscriptions sent to the graphQL server")
            .init();
        counter
    };
    static ref REQUEST_DURATION: ValueRecorder<u64> = {
        let meter = global::meter(NAME);
        let observer = meter
            .u64_value_recorder("graphql_request_duration")
            .with_description("duration of successful graphql queries in milliseconds")
            .with_unit(Unit::new("milliseconds"))
            .init();
        observer
    };
    static ref REQUEST_ERRORS: Counter<u64> = {
        let meter = global::meter(NAME);
        let counter = meter
            .u64_counter("graphql_request_errors")
            .with_description(
                "total number of graphQL queries resulting in an error being returned",
            )
            .init();
        counter
    };
}

const TARGET: &str = "async_graphql::graphql";
const NAME: &str = "graphql";
const QUERY_KEY: Key = Key::from_static_str("query_name");
const QUERY_TYPE_KEY: Key = Key::from_static_str("query_type");
const RETURN_TYPE_KEY: Key = Key::from_static_str("return_type");

pub struct OpenTelemetry;
pub struct OpenTelemetryExtension {
    start: Instant,
}

impl Default for OpenTelemetryExtension {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl ExtensionFactory for OpenTelemetry {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(OpenTelemetryExtension::default())
    }
}

#[async_trait::async_trait]
impl Extension for OpenTelemetryExtension {
    async fn request(&self, ctx: &ExtensionContext<'_>, next: NextRequest<'_>) -> Response {
        REQUESTS.add(1, &[]);
        next.run(ctx)
            .instrument(span!(target: TARGET, Level::INFO, "request"))
            .await
    }

    fn subscribe<'s>(
        &self,
        ctx: &ExtensionContext<'_>,
        stream: BoxStream<'s, Response>,
        next: NextSubscribe<'_>,
    ) -> BoxStream<'s, Response> {
        SUBSCRIPTIONS.add(1, &[]);
        Box::pin(
            next.run(ctx, stream)
                .instrument(span!(target: TARGET, Level::INFO, "subscribe")),
        )
    }

    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> ServerResult<ExecutableDocument> {
        let span = span!(target: TARGET, Level::INFO, "parse", source = query);
        tracing::trace!(parent: &span, source = query, "parsing received query");
        next.run(ctx, query, variables).instrument(span).await
    }

    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<ValidationResult, Vec<ServerError>> {
        let span = span!(target: TARGET, Level::INFO, "validation");
        next.run(ctx).instrument(span).await
    }

    async fn execute(
        &self,
        ctx: &ExtensionContext<'_>,
        operation_name: Option<&str>,
        next: NextExecute<'_>,
    ) -> Response {
        let span = span!(target: TARGET, Level::INFO, "execute");
        next.run(ctx, operation_name).instrument(span).await
    }

    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<Value>> {
        let path = info.path_node.to_string();
        let parent_type = info.parent_type.to_string();
        let return_type = info.return_type.to_string();
        let span = span!(
            target: TARGET,
            Level::INFO,
            "field",
            %path,
            %parent_type,
            %return_type
        );
        let result = next.run(ctx, info)
            .instrument(span)
            .map_err(|err| {
                REQUEST_ERRORS.add(1, &[QUERY_KEY.string(path.clone()), QUERY_TYPE_KEY.string(parent_type.clone()), RETURN_TYPE_KEY.string(return_type.clone())]);
                tracing::error!(target: TARGET, error = %err.message, extensions = ?&err.extensions);
                err
            })
            .await;
        let duration = Instant::now() - self.start;
        // This cast should be fine, because if this request duration overflows an u64, we have
        // bigger issues
        REQUEST_DURATION.record(
            duration.as_millis() as u64,
            &[
                QUERY_KEY.string(path),
                QUERY_TYPE_KEY.string(parent_type),
                RETURN_TYPE_KEY.string(return_type),
            ],
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::*;

    struct QueryRoot;

    #[Object]
    impl QueryRoot {
        pub async fn get_jane(&self) -> Query {
            Query {
                id: 100,
                details: SubQuery {
                    name: "Jane".to_owned(),
                },
            }
        }
    }

    #[derive(SimpleObject)]
    struct Query {
        id: i32,
        details: SubQuery,
    }

    #[derive(SimpleObject)]
    struct SubQuery {
        name: String,
    }

    #[tokio::test]
    async fn basic_test() {
        let schema = Schema::build(QueryRoot, EmptyMutation, EmptySubscription)
            .extension(OpenTelemetry)
            .finish();

        let query = r#"
                query {
                    getJane {
                        id
                        details {
                            name
                        }
                    }
                }
            "#;

        let request = Request::new(query);
        schema.execute(request).await;
    }
}
