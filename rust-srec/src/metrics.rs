<![CDATA[
use lazy_static::lazy_static;
use prometheus::{register_gauge, register_int_counter, Gauge, IntCounter};

lazy_static! {
    pub static ref ACTIVE_DOWNLOADS: Gauge =
        register_gauge!("active_downloads", "Number of currently active downloads").unwrap();
    pub static ref CPU_USAGE: Gauge = register_gauge!("cpu_usage", "Current CPU usage in percent").unwrap();
    pub static ref MEMORY_USAGE: Gauge =
        register_gauge!("memory_usage", "Current memory usage in bytes").unwrap();
    pub static ref PIPELINE_THROUGHPUT: IntCounter =
        register_int_counter!("pipeline_throughput", "Total number of items processed by the pipeline").unwrap();
    pub static ref DOWNLOAD_SPEED: Gauge =
        register_gauge!("download_speed", "Download speed in bytes per second").unwrap();
}
]]>
<![CDATA[
use sysinfo::{System, SystemExt};
use tokio::time::{self, Duration};

pub fn start_metrics_collection() {
    let mut sys = System::new_all();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            sys.refresh_all();
            CPU_USAGE.set(sys.global_cpu_info().cpu_usage() as f64);
            MEMORY_USAGE.set(sys.used_memory() as f64);
        }
    });
}
]]>