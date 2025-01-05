use std::time::Duration;

use crate::config::MetricConfig;
use color_eyre::eyre::Result;
use thiserror::Error;

pub mod dns;
pub mod hls;
pub mod icmp;

#[derive(Error, Debug)]
pub enum CollectorErrors {
    #[error("Failed to handle measurement for {0}: {1}")]
    Measure(String, String),
}

pub trait Collector {
    type Config;
    type Response;

    fn new(config: Self::Config) -> Self
    where
        Self: Sized;
    fn register_metrics(&self);
    async fn perform_request(&self) -> Result<Self::Response>;
    fn get_frequency(&self) -> Duration;

    fn handle_response(&self, response: Self::Response) -> Result<(), CollectorErrors>;

    async fn run(&self) -> eyre::Result<()> {
        self.register_metrics();

        loop {
            let response = self.perform_request().await?;
            self.handle_response(response)?;

            tokio::time::sleep(self.get_frequency()).await;
        }
    }
}
