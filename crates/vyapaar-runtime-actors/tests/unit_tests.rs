use std::time::Duration;

use vyapaar_runtime_actors::MetricsCollector;

#[test]
fn metrics_collector_records_and_exports_prometheus() {
    let metrics = MetricsCollector::new().expect("metrics collector creation");
    metrics.set_mailbox_queue_depth("market_data", 5);
    metrics.record_actor_latency("strategy", Duration::from_micros(650));
    metrics.record_backpressure_drop("market_data", "coalesced");
    metrics.record_actor_error("strategy", "timeout");

    let exported = metrics.export_metrics().expect("export prometheus metrics");
    assert!(exported.contains("actor_mailbox_queue_depth"));
    assert!(exported.contains("actor_processing_latency_seconds"));
    assert!(exported.contains("actor_backpressure_drops_total"));
    assert!(exported.contains("actor_errors_total"));
}
