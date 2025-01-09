use color_eyre::eyre::Result;
use std::time::Duration;
use thiserror::Error;

pub mod dns;
pub mod hls;
pub mod icmp;

#[derive(Error, Debug)]
pub enum CollectorErrors {
    #[error("Failed to measure {metric}: {reason}")]
    Measurement { metric: String, reason: String },
    #[error("Request timeout after {0:?}")]
    Timeout(Duration),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Failed to get node info for {0}")]
    MissingNodeInfo(String),
    #[error("Missing crucial data for {0} - {1}")]
    MissingData(String, &'static str),
}

/// A trait for implementing metric collectors
///
/// Collectors are responsible for gathering metrics at regular intervals
/// and processing the results.
pub trait Collector {
    type Config;
    type Response;

    /// Creates a new instance of the collector
    fn new(config: &'static Self::Config) -> Self
    where
        Self: Sized;

    /// Registers metrics with the metrics system
    fn register_metrics(&self);
    /// Performs the actual metric collection request
    async fn perform_request(&self) -> Result<Self::Response>;

    /// Returns the frequency at which this collector should run
    fn get_frequency(&self) -> Duration;

    /// Handles the response from a successful request
    fn handle_response(&self, response: Self::Response) -> Result<(), CollectorErrors>;

    /// Handles any errors that occur during collection
    fn handle_errors(&self, error: CollectorErrors) -> Result<()> {
        tracing::error!(?error, "Failed to handle error");
        Ok(())
    }

    /// Runs the collector in a loop with proper error handling and shutdown capability
    async fn run(&self) -> Result<()> {
        self.register_metrics();

        loop {
            let request_future = self.perform_request();
            let timeout_duration = Duration::from_secs(60);

            match tokio::time::timeout(timeout_duration, request_future).await {
                Ok(result) => match result {
                    Ok(response) => {
                        if let Err(e) = self.handle_response(response) {
                            self.handle_errors(e)?;
                        }
                    }
                    Err(e) => {
                        self.handle_errors(CollectorErrors::Measurement {
                            metric: "unknown".to_string(),
                            reason: e.to_string(),
                        })?;
                    }
                },
                Err(_) => {
                    self.handle_errors(CollectorErrors::Timeout(timeout_duration))?;
                }
            }

            tokio::time::sleep(self.get_frequency()).await;
        }
    }
}
