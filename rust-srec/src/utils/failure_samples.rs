//! Bounded runtime diagnostics retained until service shutdown.

use std::collections::VecDeque;

use super::text::truncate_bytes;

const MAX_SAMPLES: usize = 128;
const MAX_SAMPLE_BYTES: usize = 2048;

/// Keeps recent failure details and counts errors whose details were evicted.
/// Shutdown-phase failures use their own complete report, never this sampler.
#[derive(Default)]
pub(crate) struct FailureSamples {
    total: u64,
    samples: VecDeque<String>,
}

impl FailureSamples {
    pub(crate) fn push(&mut self, failure: String) {
        self.total = self.total.saturating_add(1);
        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
        }
        let sample = if failure.len() > MAX_SAMPLE_BYTES {
            truncate_bytes(&failure, MAX_SAMPLE_BYTES - '…'.len_utf8())
        } else {
            failure
        };
        self.samples.push_back(sample);
    }

    pub(crate) fn into_report(self) -> Vec<String> {
        let omitted = self.total.saturating_sub(self.samples.len() as u64);
        let mut report = Vec::with_capacity(self.samples.len() + usize::from(omitted > 0));
        if omitted > 0 {
            report.push(format!(
                "{omitted} earlier runtime failures omitted ({} total; retaining the latest {} samples)",
                self.total,
                self.samples.len()
            ));
        }
        report.extend(self.samples);
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_recent_utf8_samples_with_bounded_bytes_and_exact_omission_count() {
        let mut failures = FailureSamples::default();
        let detail = "弹幕".repeat(MAX_SAMPLE_BYTES);
        for index in 0..MAX_SAMPLES * 3 {
            failures.push(format!("failure {index}: {detail}"));
        }
        let report = failures.into_report();
        assert_eq!(report.len(), MAX_SAMPLES + 1);
        assert_eq!(
            report[0],
            "256 earlier runtime failures omitted (384 total; retaining the latest 128 samples)"
        );
        assert!(report[1].starts_with("failure 256:"));
        assert!(report.last().unwrap().starts_with("failure 383:"));
        assert!(
            report[1..]
                .iter()
                .all(|sample| sample.len() <= MAX_SAMPLE_BYTES && sample.ends_with('…'))
        );

        let mut one = FailureSamples::default();
        one.push("one failure".to_owned());
        assert_eq!(one.into_report(), ["one failure"]);
        assert!(FailureSamples::default().into_report().is_empty());
    }
}
