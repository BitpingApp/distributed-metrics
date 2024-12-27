use crate::config::MetricConfig;
use color_eyre::eyre::Result;

pub mod dns;
pub mod hls;
pub mod icmp;
pub mod runner;

pub trait Collector {
    fn new(config: MetricConfig) -> Self
    where
        Self: Sized;
    fn register_metrics(&self);
    fn get_config(&self) -> &MetricConfig;
    async fn perform_request(&self) -> Result<()>;
}
