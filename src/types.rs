use std::collections::BTreeMap;
use std::ops::Deref;

use chrono::{DateTime, Utc};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use tracing::Span;

use async_graphql::extensions::{Extension, ExtensionFactory};
use async_graphql::QueryPathNode;

/// The base type for initialising the extension in your application
///
/// This should be attached to the schema when generating it
/// # Examples
///
/// ```no_run
/// use async_graphql::*;
/// use async_graphql_telemetry_extension::{OpenTelemetryConfig, OpenTelemetryExtension};
/// use tracing::{span, Level};
///
/// #[derive(SimpleObject)]
/// struct Query {
///     value: i32,
/// }
///
/// let schema = Schema::build(Query { value: 100 }, EmptyMutation, EmptySubscription).
///     extension(OpenTelemetryExtension)
///     .finish();
/// ```

#[derive(Default)]
pub struct OpenTelemetryExtension;

impl ExtensionFactory for OpenTelemetryExtension {
    fn create(&self) -> Box<dyn Extension> {
        Box::new(OpenTelemetry {
            metrics: Metrics {
                start_time: Utc::now(),
                end_time: Utc::now(),
                resolves: Default::default(),
            },
            traces: Default::default(),
            fields: Default::default(),
            query_name: None,
            query_root: None,
        })
    }
}

pub(crate) struct OpenTelemetry {
    pub(crate) metrics: Metrics,
    pub(crate) traces: Traces,
    pub(crate) fields: BTreeMap<usize, TelemetryData>,
    pub(crate) query_name: Option<String>,
    pub(crate) query_root: Option<String>,
}

pub(crate) struct Metrics {
    pub(crate) start_time: DateTime<Utc>,
    pub(crate) end_time: DateTime<Utc>,
    pub(crate) resolves: Vec<ResolveStat>,
}

#[derive(Default)]
pub(crate) struct Traces {
    pub(crate) root: Option<Span>,
    pub(crate) parse: Option<Span>,
    pub(crate) validation: Option<Span>,
    pub(crate) execute: Option<Span>,
}

pub(crate) struct TelemetryData {
    pub(crate) span: Span,
    pub(crate) metrics: PendingResolve,
}

impl TelemetryData {
    pub fn new<'a>(
        span: Span,
        path_node: &'a QueryPathNode<'a>,
        parent_type: String,
        return_type: String,
    ) -> Self {
        Self {
            metrics: PendingResolve {
                path: path_node.to_string_vec(),
                field_name: path_node.field_name().to_string(),
                parent_type,
                return_type,
                start_time: Utc::now(),
            },
            span,
        }
    }
}

/// Tracing extension configuration for each request.
///
/// Should be injected into the GraphQL `Context`
///
/// # Examples
///
/// ```no_run
/// use async_graphql::*;
/// use async_graphql_telemetry_extension::{OpenTelemetryConfig, OpenTelemetryExtension};
/// use tracing::{span, Level};
///
/// #[derive(SimpleObject)]
/// struct Query {
///     value: i32,
/// }
///
/// let schema = Schema::build(Query { value: 100 }, EmptyMutation, EmptySubscription).
///     extension(OpenTelemetryExtension)
///     .finish();
///
/// let root_span = span!(
///     parent: None,
///     Level::INFO,
///     "request root"
/// );
///
/// tokio::task::block_in_place(|| {
///     async move {
///         let otel_ext = OpenTelemetryConfig::default()
///             .parent_span(root_span)
///             .enable_apollo_tracing(false);
///         let request = Request::new("{ value }")
///             .data(otel_ext);
///         schema.execute(request).await;
///     }
/// });
/// ```
pub struct OpenTelemetryConfig {
    /// Use a span as the parent node of the entire query.
    pub parent: Option<Span>,
    pub return_tracing_data_to_client: bool,
}

impl Default for OpenTelemetryConfig {
    fn default() -> Self {
        Self {
            parent: None,
            return_tracing_data_to_client: true,
        }
    }
}

impl OpenTelemetryConfig {
    /// Use the provided span as the parent node of the entire query.
    pub fn parent_span(mut self, span: Span) -> Self {
        self.parent = Some(span);
        self
    }

    /// Set this to enable/disable whether apollo tracing is returned to the client
    ///
    /// ## Default
    ///
    /// By default this is set to true
    pub fn enable_apollo_tracing(mut self, enable: bool) -> Self {
        self.return_tracing_data_to_client = enable;
        self
    }
}

#[derive(Debug)]
pub(crate) struct PendingResolve {
    pub(crate) path: Vec<String>,
    pub(crate) field_name: String,
    pub(crate) parent_type: String,
    pub(crate) return_type: String,
    pub(crate) start_time: DateTime<Utc>,
}

#[derive(Debug)]
pub(crate) struct ResolveStat {
    pub(crate) pending_resolve: PendingResolve,
    pub(crate) end_time: DateTime<Utc>,
    pub(crate) start_offset: i64,
}

impl Deref for ResolveStat {
    type Target = PendingResolve;

    fn deref(&self) -> &Self::Target {
        &self.pending_resolve
    }
}

impl Serialize for ResolveStat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("path", &self.path)?;
        map.serialize_entry("fieldName", &self.field_name)?;
        map.serialize_entry("parentType", &self.parent_type)?;
        map.serialize_entry("returnType", &self.return_type)?;
        map.serialize_entry("startOffset", &self.start_offset)?;
        map.serialize_entry(
            "duration",
            &(self.end_time - self.start_time).num_nanoseconds(),
        )?;
        map.end()
    }
}
