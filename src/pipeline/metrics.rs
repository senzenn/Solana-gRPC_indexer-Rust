use std::sync::OnceLock;

use metrics::{counter, describe_counter, describe_gauge, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static PROMETHEUS: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus recorder once and return the scrape handle.
pub fn init() -> &'static PrometheusHandle {
    PROMETHEUS.get_or_init(|| {
        describe_counter!(
            "indexer_events_total",
            "Total number of events decoded and stored"
        );
        describe_gauge!(
            "indexer_slot_current",
            "Latest slot observed by the indexer"
        );
        describe_gauge!(
            "indexer_lag_slots",
            "Slots behind chain head when computable"
        );
        describe_gauge!("indexer_open_gaps", "Number of unfilled slot gaps");
        describe_counter!(
            "indexer_geyser_reconnects_total",
            "Total geyser stream reconnect attempts"
        );
        describe_counter!(
            "indexer_geyser_errors_total",
            "Total geyser stream errors before reconnect"
        );

        PrometheusBuilder::new()
            .install_recorder()
            .expect("install prometheus metrics recorder")
    })
}

pub fn render() -> String {
    init().render()
}

pub fn record_event_stored() {
    init();
    counter!("indexer_events_total").increment(1);
}

pub fn update_live_stats(slot: u64, lag_slots: u64, open_gaps: i64) {
    init();
    gauge!("indexer_slot_current").set(slot as f64);
    gauge!("indexer_lag_slots").set(lag_slots as f64);
    gauge!("indexer_open_gaps").set(open_gaps as f64);
}

pub fn record_reconnect() {
    init();
    counter!("indexer_geyser_reconnects_total").increment(1);
}

pub fn record_geyser_error() {
    init();
    counter!("indexer_geyser_errors_total").increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prometheus_render_includes_metric_names() {
        record_event_stored();
        record_reconnect();
        record_geyser_error();
        update_live_stats(42, 3, 1);

        let body = render();
        assert!(body.contains("indexer_events_total"));
        assert!(body.contains("indexer_slot_current"));
        assert!(body.contains("indexer_lag_slots"));
        assert!(body.contains("indexer_open_gaps"));
        assert!(body.contains("indexer_geyser_reconnects_total"));
        assert!(body.contains("indexer_geyser_errors_total"));
    }
}
