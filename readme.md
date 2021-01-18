# Async GraphQL Telemetry Extension

This repo just contains a very small library which implements the [Async GraphQL](https://github.com/async-graphql/async-graphql) `Extensions` trait. It does so in a manner that mimics traditional `HTTP` middleware to
generate OpenTelemetry compatible traces and metrics.

This is very much a work in progress (especially given how early stage the Open
Telemetry and Tracing APIs are), and at least initially is completely tailored
to some of my own use cases. However if it's useful for you then please go ahead
and make some use of it.

Any code found within this repository is licensed under the same terms as the main `Async GraphQL` crate.

## License

Licensed under either of

- Apache License, Version 2.0, (./LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license (./LICENSE-MIT or http://opensource.org/licenses/MIT) at your option.
