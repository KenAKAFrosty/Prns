use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::IsTerminal;
#[cfg(feature = "otlp")]
use std::time::Duration;

#[cfg(feature = "otlp")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otlp")]
use opentelemetry::KeyValue;
#[cfg(feature = "otlp")]
use opentelemetry_otlp::{Protocol, WithExportConfig};
#[cfg(feature = "otlp")]
use opentelemetry_sdk::metrics::SdkMeterProvider;
#[cfg(feature = "otlp")]
use opentelemetry_sdk::trace::{
    BatchConfigBuilder, BatchSpanProcessor, Sampler, SdkTracerProvider,
};
#[cfg(feature = "otlp")]
use opentelemetry_sdk::Resource;
use tracing::subscriber::SetGlobalDefaultError;
#[cfg(feature = "otlp")]
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::filter::ParseError;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use crate::cli::LogFormat;
use personal_rns::config::LoggingPlan;

#[cfg(feature = "otlp")]
mod metrics;
mod progress;

pub(crate) use progress::StateRestoreProgress;

#[cfg(feature = "otlp")]
const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum ObservabilityError {
    Environment(&'static str),
    Filter(ParseError),
    Logger(log::SetLoggerError),
    Subscriber(SetGlobalDefaultError),
    #[cfg(feature = "otlp")]
    Otlp(opentelemetry_otlp::ExporterBuildError),
}

impl Display for ObservabilityError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Environment(name) => write!(formatter, "{name} is not valid Unicode"),
            Self::Filter(error) => write!(formatter, "invalid RUST_LOG filter: {error}"),
            Self::Logger(error) => write!(formatter, "log bridge initialization failed: {error}"),
            Self::Subscriber(error) => {
                write!(
                    formatter,
                    "tracing subscriber initialization failed: {error}"
                )
            }
            #[cfg(feature = "otlp")]
            Self::Otlp(error) => write!(formatter, "OTLP exporter initialization failed: {error}"),
        }
    }
}

impl Error for ObservabilityError {}

pub struct ObservabilityGuard {
    local_terminal: bool,
    #[cfg(feature = "otlp")]
    tracer_provider: Option<SdkTracerProvider>,
    #[cfg(feature = "otlp")]
    meter_provider: Option<SdkMeterProvider>,
}

impl ObservabilityGuard {
    pub(crate) fn state_restore_progress(&self) -> Option<StateRestoreProgress> {
        self.local_terminal.then(StateRestoreProgress::new)
    }

    #[cfg(feature = "otlp")]
    pub(crate) fn metrics_reporter(&self) -> Option<metrics::MetricsReporter> {
        self.meter_provider
            .as_ref()
            .map(metrics::MetricsReporter::new)
    }

    pub async fn shutdown(self) {
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.meter_provider {
            match tokio::task::spawn_blocking(move || provider.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(event = "otlp_metrics_shutdown_failed", error = %error);
                }
                Err(error) => {
                    tracing::warn!(event = "otlp_metrics_shutdown_task_failed", error = %error);
                }
            }
        }
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.tracer_provider {
            match tokio::task::spawn_blocking(move || {
                provider.shutdown_with_timeout(EXPORT_TIMEOUT)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(event = "otlp_shutdown_failed", error = %error);
                }
                Err(error) => {
                    tracing::warn!(event = "otlp_shutdown_task_failed", error = %error);
                }
            }
        }
        #[cfg(not(feature = "otlp"))]
        let _ = self;
    }
}

fn optional_env(name: &'static str) -> Result<Option<String>, ObservabilityError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ObservabilityError::Environment(name)),
    }
}

#[cfg(feature = "otlp")]
fn otlp_requested_from(
    endpoint: Option<&str>,
    traces_endpoint: Option<&str>,
    sdk_disabled: Option<&str>,
) -> bool {
    let disabled = sdk_disabled.is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let has_endpoint = [endpoint, traces_endpoint]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty());
    has_endpoint && !disabled
}

#[cfg(feature = "otlp")]
fn build_tracer_provider() -> Result<Option<SdkTracerProvider>, ObservabilityError> {
    let endpoint = optional_env("OTEL_EXPORTER_OTLP_ENDPOINT")?;
    let traces_endpoint = optional_env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")?;
    let sdk_disabled = optional_env("OTEL_SDK_DISABLED")?;
    if !otlp_requested_from(
        endpoint.as_deref(),
        traces_endpoint.as_deref(),
        sdk_disabled.as_deref(),
    ) {
        return Ok(None);
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(ObservabilityError::Otlp)?;
    let batch = BatchSpanProcessor::builder(exporter)
        .with_batch_config(
            BatchConfigBuilder::default()
                .with_max_queue_size(2_048)
                .with_max_export_batch_size(512)
                .with_scheduled_delay(Duration::from_secs(5))
                .build(),
        )
        .build();
    let resource = telemetry_resource()?;
    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_span_processor(batch);
    let provider = if optional_env("OTEL_TRACES_SAMPLER")?.is_none() {
        provider.with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            0.1,
        ))))
    } else {
        provider
    };
    Ok(Some(provider.build()))
}

#[cfg(feature = "otlp")]
fn build_meter_provider() -> Result<Option<SdkMeterProvider>, ObservabilityError> {
    let endpoint = optional_env("OTEL_EXPORTER_OTLP_ENDPOINT")?;
    let metrics_endpoint = optional_env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")?;
    let sdk_disabled = optional_env("OTEL_SDK_DISABLED")?;
    if !otlp_requested_from(
        endpoint.as_deref(),
        metrics_endpoint.as_deref(),
        sdk_disabled.as_deref(),
    ) {
        return Ok(None);
    }

    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(ObservabilityError::Otlp)?;
    Ok(Some(
        SdkMeterProvider::builder()
            .with_resource(telemetry_resource()?)
            .with_periodic_exporter(exporter)
            .build(),
    ))
}

#[cfg(feature = "otlp")]
fn telemetry_resource() -> Result<Resource, ObservabilityError> {
    let service_name = optional_env("OTEL_SERVICE_NAME")?
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "prnsd".to_string());
    Ok(Resource::builder()
        .with_service_name(service_name)
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build())
}

pub fn init(
    format: LogFormat,
    logging: LoggingPlan,
) -> Result<ObservabilityGuard, ObservabilityError> {
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let filter = optional_env("RUST_LOG")?
        .as_deref()
        .map_or_else(
            || tracing_subscriber::EnvFilter::try_new(filter_for_level(logging.level.get())),
            tracing_subscriber::EnvFilter::try_new,
        )
        .map_err(ObservabilityError::Filter)?;
    tracing_log::LogTracer::builder()
        .with_max_level(log::LevelFilter::Trace)
        .init()
        .map_err(ObservabilityError::Logger)?;

    #[cfg(feature = "otlp")]
    let tracer_provider = build_tracer_provider()?;
    #[cfg(feature = "otlp")]
    let meter_provider = build_meter_provider()?;
    #[cfg(feature = "otlp")]
    let otlp_layer = tracer_provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("prnsd"))
            .with_filter(LevelFilter::DEBUG)
    });

    match format {
        LogFormat::Human => {
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(stderr_is_terminal)
                .with_target(true)
                .with_writer(std::io::stderr);
            if logging.timestamps {
                let layer = layer.with_filter(filter);
                #[cfg(feature = "otlp")]
                let subscriber = tracing_subscriber::registry().with(otlp_layer).with(layer);
                #[cfg(not(feature = "otlp"))]
                let subscriber = tracing_subscriber::registry().with(layer);
                tracing::subscriber::set_global_default(subscriber)
                    .map_err(ObservabilityError::Subscriber)?;
            } else {
                let layer = layer.without_time().with_filter(filter);
                #[cfg(feature = "otlp")]
                let subscriber = tracing_subscriber::registry().with(otlp_layer).with(layer);
                #[cfg(not(feature = "otlp"))]
                let subscriber = tracing_subscriber::registry().with(layer);
                tracing::subscriber::set_global_default(subscriber)
                    .map_err(ObservabilityError::Subscriber)?;
            }
        }
        LogFormat::Json => {
            let layer = tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(true)
                .with_span_list(true)
                .with_target(true)
                .with_writer(std::io::stderr);
            if logging.timestamps {
                let layer = layer.with_filter(filter);
                #[cfg(feature = "otlp")]
                let subscriber = tracing_subscriber::registry().with(otlp_layer).with(layer);
                #[cfg(not(feature = "otlp"))]
                let subscriber = tracing_subscriber::registry().with(layer);
                tracing::subscriber::set_global_default(subscriber)
                    .map_err(ObservabilityError::Subscriber)?;
            } else {
                let layer = layer.without_time().with_filter(filter);
                #[cfg(feature = "otlp")]
                let subscriber = tracing_subscriber::registry().with(otlp_layer).with(layer);
                #[cfg(not(feature = "otlp"))]
                let subscriber = tracing_subscriber::registry().with(layer);
                tracing::subscriber::set_global_default(subscriber)
                    .map_err(ObservabilityError::Subscriber)?;
            }
        }
    }

    Ok(ObservabilityGuard {
        local_terminal: format == LogFormat::Human && stderr_is_terminal,
        #[cfg(feature = "otlp")]
        tracer_provider,
        #[cfg(feature = "otlp")]
        meter_provider,
    })
}

fn filter_for_level(level: u8) -> &'static str {
    match level {
        0 | 1 => "error",
        2 => "warn",
        3 | 4 => "info",
        5 | 6 => "debug",
        _ => "trace",
    }
}

#[cfg(test)]
mod level_tests {
    use super::filter_for_level;

    #[test]
    fn stock_log_levels_map_to_rust_filters() {
        assert_eq!(filter_for_level(0), "error");
        assert_eq!(filter_for_level(1), "error");
        assert_eq!(filter_for_level(2), "warn");
        assert_eq!(filter_for_level(3), "info");
        assert_eq!(filter_for_level(4), "info");
        assert_eq!(filter_for_level(5), "debug");
        assert_eq!(filter_for_level(6), "debug");
        assert_eq!(filter_for_level(7), "trace");
    }
}

#[cfg(all(test, feature = "otlp"))]
mod tests {
    use super::otlp_requested_from;

    #[test]
    fn otlp_requires_an_endpoint_and_honors_sdk_disable() {
        assert!(!otlp_requested_from(None, None, None));
        assert!(!otlp_requested_from(Some("  "), None, None));
        assert!(otlp_requested_from(
            Some("http://collector:4318"),
            None,
            None
        ));
        assert!(otlp_requested_from(
            None,
            Some("http://collector:4318/v1/traces"),
            None
        ));
        assert!(!otlp_requested_from(
            Some("http://collector:4318"),
            None,
            Some("TRUE")
        ));
    }
}
