use std::time::Duration;

use crate::config::MetricConfig;
use color_eyre::eyre::Result;

pub mod dns;
pub mod hls;
pub mod icmp;

pub trait Collector {
    type Config;

    fn new(config: Self::Config) -> Self
    where
        Self: Sized;
    fn register_metrics(&self);
    async fn perform_request(&self) -> Result<()>;
    fn get_frequency(&self) -> Duration;

    async fn run(&self) -> eyre::Result<()> {
        self.register_metrics();

        loop {
            self.perform_request().await?;
            tokio::time::sleep(self.get_frequency()).await;
        }
    }
}
