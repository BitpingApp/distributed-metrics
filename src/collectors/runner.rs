use super::Collector;

pub async fn run_collector<T: Collector>(collector: T) -> eyre::Result<()> {
    collector.register_metrics();

    let frequency = tokio::time::Duration::from_millis(collector.get_config().frequency_ms);

    loop {
        collector.perform_request().await?;
        tokio::time::sleep(frequency).await;
    }
}
