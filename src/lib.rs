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
//! - Apollo Tracing
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

use chrono::Utc;
use opentelemetry::{
    global,
    metrics::{Counter, ValueRecorder},
    Key,
};

use async_graphql::extensions::{Extension, ExtensionContext, ResolveInfo};
use async_graphql::parser::types::ExecutableDocument;
use async_graphql::{
    value, Request, ServerError, ServerResult, ValidationResult, Value, Variables,
};
use lazy_static::lazy_static;
use tracing::{span, Level};

mod types;

pub use types::*;

lazy_static! {
    static ref HTTP_REQUESTS: Counter<u64> = {
        let meter = global::meter("graphql");
        let counter = meter
            .u64_counter("graphql_requests")
            .with_description("total number of HTTP requests sent to the graphQL server")
            .init();
        counter
    };
    static ref HTTP_REQUEST_DURATION: ValueRecorder<f64> = {
        let meter = global::meter("graphql");
        let observer = meter
            .f64_value_recorder("graphql_request_duration")
            .with_description("duration of successful graphql queries in milliseconds")
            .init();
        observer
    };
    static ref HTTP_REQUESTS_ERRORS: Counter<u64> = {
        let meter = global::meter("graphql");
        let counter = meter
            .u64_counter("graphql_request_errors")
            .with_description(
                "total number of graphQL queries resulting in an error being returned",
            )
            .init();
        counter
    };
}

macro_rules! prefix_context {
    ($context:literal) => {
        concat!("graphql::", $context)
    };
}

const TARGET: &str = "async_graphql::graphql";
const ROOT: Key = Key::from_static_str("root");
const QUERY: Key = Key::from_static_str("query");

#[async_trait::async_trait]
impl Extension for OpenTelemetry {
    fn name(&self) -> Option<&'static str> {
        Some("tracing")
    }

    async fn prepare_request(
        &mut self,
        ctx: &ExtensionContext<'_>,
        request: Request,
    ) -> ServerResult<Request> {
        let parent_span = ctx
            .data_opt::<OpenTelemetryConfig>()
            .and_then(|cfg| cfg.parent.as_ref());

        let root_span = match parent_span {
            Some(parent) => span!(
                target: TARGET,
                parent: parent,
                Level::INFO,
                prefix_context!("request")
            ),
            None => span!(
                target: TARGET,
                parent: None,
                Level::INFO,
                prefix_context!("request")
            ),
        };

        root_span.with_subscriber(|(id, d)| d.enter(id));
        self.traces.root.replace(root_span);
        Ok(request)
    }

    fn parse_start(
        &mut self,
        _ctx: &ExtensionContext<'_>,
        _query_source: &str,
        _variables: &Variables,
    ) {
        if let Some(ref root) = self.traces.root {
            let parse_span = span!(
                target: TARGET,
                parent: root,
                Level::DEBUG,
                prefix_context!("parse")
            );

            parse_span.with_subscriber(|(id, d)| d.enter(id));
            self.traces.parse.replace(parse_span);
            self.metrics.start_time = Utc::now();
        }
    }

    fn validation_start(&mut self, _ctx: &ExtensionContext<'_>) {
        if let Some(parent) = &self.traces.root {
            let validation_span = span!(
                target: TARGET,
                parent: parent,
                Level::DEBUG,
                prefix_context!("validation")
            );
            validation_span.with_subscriber(|(id, d)| d.enter(id));
            self.traces.validation.replace(validation_span);
        }
    }

    fn parse_end(&mut self, _ctx: &ExtensionContext<'_>, _document: &ExecutableDocument) {
        self.traces
            .parse
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
    }

    fn validation_end(&mut self, _ctx: &ExtensionContext<'_>, _result: &ValidationResult) {
        self.traces
            .validation
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
    }

    fn execution_start(&mut self, _ctx: &ExtensionContext<'_>) {
        let execute_span = if let Some(parent) = &self.traces.root {
            span!(
                target: TARGET,
                parent: parent,
                Level::DEBUG,
                prefix_context!("execute")
            )
        } else {
            // For every step of the subscription stream.
            span!(
                target: TARGET,
                parent: None,
                Level::DEBUG,
                prefix_context!("execute")
            )
        };

        execute_span.with_subscriber(|(id, d)| d.enter(id));
        self.traces.execute.replace(execute_span);
    }

    fn execution_end(&mut self, _ctx: &ExtensionContext<'_>) {
        self.traces
            .execute
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
        self.traces
            .root
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
        self.metrics.end_time = Utc::now();
    }

    fn resolve_start(&mut self, _ctx: &ExtensionContext<'_>, info: &ResolveInfo<'_>) {
        let parent_span = match info.resolve_id.parent {
            Some(parent_id) if parent_id > 0 => self
                .fields
                .get(&parent_id)
                .map(|telemetry_data| &telemetry_data.span),
            _ => self.traces.execute.as_ref(),
        };

        if let Some(parent_span) = parent_span {
            if self.query_name.is_none() {
                self.query_name = Some(info.path_node.to_string());
            }
            if self.query_root.is_none() {
                self.query_root = Some(info.parent_type.to_string());
            }

            let span = span!(
                target: TARGET,
                parent: parent_span,
                Level::TRACE,
                prefix_context!("field_resolver"),
                graphql_field_id = %info.resolve_id.current,
                graphql_path = %info.path_node,
                graphql_parent_type = %info.parent_type,
                graphql_return_type = %info.return_type,
            );

            span.with_subscriber(|(id, d)| d.enter(id));

            let telemetry_data = TelemetryData::new(
                span,
                info.path_node,
                info.parent_type.to_string(),
                info.return_type.to_string(),
            );
            self.fields.insert(info.resolve_id.current, telemetry_data);
        }
    }

    fn resolve_end(&mut self, _ctx: &ExtensionContext<'_>, info: &ResolveInfo<'_>) {
        if let Some(telemetry_data) = self.fields.remove(&info.resolve_id.current) {
            telemetry_data.span.with_subscriber(|(id, d)| d.exit(id));
            let pending_resolve = telemetry_data.metrics;
            let start_offset = (pending_resolve.start_time - self.metrics.start_time)
                .num_nanoseconds()
                .unwrap();
            self.metrics.resolves.push(ResolveStat {
                pending_resolve,
                start_offset,
                end_time: Utc::now(),
            });
        }
    }

    fn error(&mut self, _ctx: &ExtensionContext<'_>, err: &ServerError) {
        let resolved_values = self.metrics.resolves.len();
        let pending_values = self.fields.len();
        let time_to_error_ms = (Utc::now() - self.metrics.start_time).num_milliseconds();
        tracing::debug!(target: TARGET, error = %err.message, error.extensions = ?err.extensions, resolved_values, pending_values, %time_to_error_ms, "Found error when resolving GraphQL field");

        for (_, TelemetryData { span, .. }) in self.fields.iter() {
            span.with_subscriber(|(id, d)| d.exit(id));
        }
        self.fields.clear();

        // These two fields should always have been populated by now,
        // if it isn't we'll have a catch all of an empty string ""
        let query_name = self.query_name.clone().unwrap_or_default();
        let query_root = self.query_root.clone().unwrap_or_default();
        HTTP_REQUESTS.add(
            1,
            &[
                QUERY.string(query_name.clone()),
                ROOT.string(query_root.clone()),
            ],
        );
        HTTP_REQUESTS_ERRORS.add(1, &[QUERY.string(query_name), ROOT.string(query_root)]);

        self.traces
            .execute
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
        self.traces
            .validation
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
        self.traces
            .parse
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
        self.traces
            .root
            .take()
            .and_then(|span| span.with_subscriber(|(id, d)| d.exit(id)));
    }

    fn result(&mut self, ctx: &ExtensionContext<'_>) -> Option<Value> {
        self.metrics
            .resolves
            .sort_by(|a, b| a.start_offset.cmp(&b.start_offset));

        let request_duration = (self.metrics.end_time - self.metrics.start_time)
            .num_nanoseconds()
            .expect("should be valid duration");
        let query_name = self.query_name.clone().unwrap_or_default();
        let query_root = self.query_root.clone().unwrap_or_default();
        HTTP_REQUESTS.add(
            1,
            &[
                QUERY.string(query_name.clone()),
                ROOT.string(query_root.clone()),
            ],
        );
        HTTP_REQUEST_DURATION.record(
            request_duration as f64 / 1_000_000.,
            &[QUERY.string(query_name), ROOT.string(query_root)],
        );

        let result = value!({
            "version": 1,
            "startTime": self.metrics.start_time.to_rfc3339(),
            "endTime": self.metrics.end_time.to_rfc3339(),
            "duration": request_duration,
            "execution": {
                "resolvers": self.metrics.resolves
            }
        });
        if let Some(cfg) = ctx.data_opt::<OpenTelemetryConfig>() {
            if !cfg.return_tracing_data_to_client {
                return None;
            }
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::extensions::TracingConfig;
    use async_graphql::*;
    use tracing::{span, Level};

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
            .extension(OpenTelemetryExtension)
            .finish();

        let root_span = span!(parent: None, Level::INFO, "span root");
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

        let request = Request::new(query).data(TracingConfig::default().parent_span(root_span));
        schema.execute(request).await;
    }
}
