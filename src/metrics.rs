//! Metrics collector for OggMux stream monitoring.
//!
//! This module provides tools for monitoring stream health, tracking
//! latency and buffer utilization, counting silence insertions, and measuring
//! other performance characteristics.

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// Statistics type for a single metric
#[derive(Clone, Debug, Default)]
pub struct MetricStats {
    /// Minimum value observed
    pub min: f64,
    /// Maximum value observed
    pub max: f64,
    /// Average value
    pub avg: f64,
    /// Number of samples
    pub samples: usize,
    /// Last value observed
    pub last: f64,
}

/// Stream metrics collected during OggMux operation.
///
/// These metrics provide insight into the health and performance
/// of the streaming process, useful for monitoring and debugging.
#[derive(Debug, Clone)]
pub struct StreamMetrics {
    /// Total bytes processed
    pub bytes_processed: usize,
    /// Total silence bytes inserted
    pub silence_bytes_inserted: usize,
    /// Number of times silence was inserted
    pub silence_insertions: usize,
    /// Number of real audio streams processed
    pub real_streams_processed: usize,
    /// Buffer utilization (as percentage of max capacity)
    pub buffer_utilization: MetricStats,
    /// Processing latency in milliseconds
    pub processing_latency_ms: MetricStats,
    /// Time since creation
    pub start_time: Instant,
    /// Last update time
    pub last_update: Instant,
}

impl Default for StreamMetrics {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            bytes_processed: 0,
            silence_bytes_inserted: 0,
            silence_insertions: 0,
            real_streams_processed: 0,
            buffer_utilization: MetricStats::default(),
            processing_latency_ms: MetricStats::default(),
            start_time: now,
            last_update: now,
        }
    }
}

impl StreamMetrics {
    /// Create a new StreamMetrics instance
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start_time: now,
            last_update: now,
            ..Default::default()
        }
    }

    /// Record a buffer utilization measurement
    pub fn record_buffer_utilization(&mut self, utilization: f64) {
        let stats = &mut self.buffer_utilization;
        if stats.samples == 0 {
            stats.min = utilization;
            stats.max = utilization;
            stats.avg = utilization;
        } else {
            stats.min = stats.min.min(utilization);
            stats.max = stats.max.max(utilization);
            stats.avg = (stats.avg * stats.samples as f64 + utilization) / (stats.samples as f64 + 1.0);
        }
        stats.samples += 1;
        stats.last = utilization;
        self.last_update = Instant::now();
    }

    /// Record a processing latency measurement
    pub fn record_processing_latency(&mut self, latency_ms: f64) {
        let stats = &mut self.processing_latency_ms;
        if stats.samples == 0 {
            stats.min = latency_ms;
            stats.max = latency_ms;
            stats.avg = latency_ms;
        } else {
            stats.min = stats.min.min(latency_ms);
            stats.max = stats.max.max(latency_ms);
            stats.avg = (stats.avg * stats.samples as f64 + latency_ms) / (stats.samples as f64 + 1.0);
        }
        stats.samples += 1;
        stats.last = latency_ms;
        self.last_update = Instant::now();
    }

    /// Add processed bytes count
    pub fn add_bytes_processed(&mut self, bytes: usize) {
        self.bytes_processed += bytes;
        self.last_update = Instant::now();
    }

    /// Add silence bytes count
    pub fn add_silence_bytes(&mut self, bytes: usize) {
        self.silence_bytes_inserted += bytes;
        self.silence_insertions += 1;
        self.last_update = Instant::now();
    }

    /// Increment real stream counter
    pub fn increment_real_streams(&mut self) {
        self.real_streams_processed += 1;
        self.last_update = Instant::now();
    }

    /// Get uptime in seconds
    pub fn uptime_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Get time since last update in seconds
    pub fn idle_seconds(&self) -> f64 {
        self.last_update.elapsed().as_secs_f64()
    }

    /// Reset all metrics
    pub fn reset(&mut self) {
        let now = Instant::now();
        *self = Self {
            start_time: self.start_time, // Keep original start time
            last_update: now,
            ..Default::default()
        };
    }

    /// Calculate and return silence percentage
    pub fn silence_percentage(&self) -> f64 {
        if self.bytes_processed == 0 {
            return 0.0;
        }
        (self.silence_bytes_inserted as f64 / self.bytes_processed as f64) * 100.0
    }

}

/// Thread-safe metrics collector.
///
/// This struct provides a shared, thread-safe interface for collecting
/// and retrieving stream metrics during OggMux operation.
#[derive(Clone, Debug)]
pub struct MetricsCollector {
    metrics: Arc<Mutex<StreamMetrics>>,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(Mutex::new(StreamMetrics::new())),
        }
    }

    /// Record buffer utilization
    pub async fn record_buffer_utilization(&self, utilization: f64) {
        let mut metrics = self.metrics.lock().await;
        metrics.record_buffer_utilization(utilization);
    }

    /// Record processing latency
    pub async fn record_processing_latency(&self, latency_ms: f64) {
        let mut metrics = self.metrics.lock().await;
        metrics.record_processing_latency(latency_ms);
    }

    /// Record processed bytes
    pub async fn add_bytes_processed(&self, bytes: usize) {
        let mut metrics = self.metrics.lock().await;
        metrics.add_bytes_processed(bytes);
    }

    /// Record silence bytes
    pub async fn add_silence_bytes(&self, bytes: usize) {
        let mut metrics = self.metrics.lock().await;
        metrics.add_silence_bytes(bytes);
    }

    /// Increment real stream counter
    pub async fn increment_real_streams(&self) {
        let mut metrics = self.metrics.lock().await;
        metrics.increment_real_streams();
    }

    /// Get a snapshot of current metrics
    pub async fn snapshot(&self) -> StreamMetrics {
        self.metrics.lock().await.clone()
    }

    /// Reset metrics
    pub async fn reset(&self) {
        let mut metrics = self.metrics.lock().await;
        metrics.reset();
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_metric_stats_updates() {
        // Test buffer utilization stats
        let mut metrics = StreamMetrics::new();
        
        // First value
        metrics.record_buffer_utilization(10.0);
        assert_eq!(metrics.buffer_utilization.min, 10.0);
        assert_eq!(metrics.buffer_utilization.max, 10.0);
        assert_eq!(metrics.buffer_utilization.avg, 10.0);
        assert_eq!(metrics.buffer_utilization.samples, 1);
        
        // Second value
        metrics.record_buffer_utilization(20.0);
        assert_eq!(metrics.buffer_utilization.min, 10.0);
        assert_eq!(metrics.buffer_utilization.max, 20.0);
        assert_eq!(metrics.buffer_utilization.avg, 15.0);
        assert_eq!(metrics.buffer_utilization.samples, 2);
        
        // Third value - lower than min
        metrics.record_buffer_utilization(5.0);
        assert_eq!(metrics.buffer_utilization.min, 5.0);
        assert_eq!(metrics.buffer_utilization.max, 20.0);
        assert!((metrics.buffer_utilization.avg - 11.6667).abs() < 0.001);
        assert_eq!(metrics.buffer_utilization.samples, 3);
    }

    #[test]
    fn test_silence_percentage() {
        let mut metrics = StreamMetrics::new();
        
        // No data
        assert_eq!(metrics.silence_percentage(), 0.0);
        
        // Some data
        metrics.add_bytes_processed(1000);
        metrics.add_silence_bytes(250);
        assert_eq!(metrics.silence_percentage(), 25.0);
        
        // More data
        metrics.add_bytes_processed(1000);
        metrics.add_silence_bytes(250);
        assert_eq!(metrics.silence_percentage(), 25.0);
    }

    #[test]
    fn test_uptime_and_idle() {
        let metrics = StreamMetrics::new();
        thread::sleep(Duration::from_millis(10));
        
        assert!(metrics.uptime_seconds() > 0.0);
        assert!(metrics.idle_seconds() > 0.0);
        
        // Idle should be same as uptime initially
        assert!((metrics.idle_seconds() - metrics.uptime_seconds()).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_metrics_collector() {
        let collector = MetricsCollector::new();
        
        // Record some metrics
        collector.record_buffer_utilization(50.0).await;
        collector.add_bytes_processed(1000).await;
        collector.add_silence_bytes(200).await;
        collector.increment_real_streams().await;
        
        // Get snapshot
        let snapshot = collector.snapshot().await;
        
        // Verify snapshot
        assert_eq!(snapshot.bytes_processed, 1000);
        assert_eq!(snapshot.silence_bytes_inserted, 200);
        assert_eq!(snapshot.silence_insertions, 1);
        assert_eq!(snapshot.real_streams_processed, 1);
        assert_eq!(snapshot.buffer_utilization.last, 50.0);
        
        // Reset and verify
        collector.reset().await;
        let new_snapshot = collector.snapshot().await;
        assert_eq!(new_snapshot.bytes_processed, 0);
        assert_eq!(new_snapshot.silence_insertions, 0);
        
        // Start time should be preserved
        assert!((new_snapshot.start_time.duration_since(snapshot.start_time)).as_nanos() < 1000);
    }
}