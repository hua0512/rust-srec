//! Input accounting shared by transfer processors.

use std::collections::HashMap;
use std::path::Path;

/// An unreadable or unavailable path must remain pending. Only a positive
/// absence result establishes that a previous transfer consumed its source.
pub(super) async fn is_confirmed_absent(path: &Path) -> bool {
    matches!(tokio::fs::try_exists(path).await, Ok(false))
}

/// Split inputs into (pending, confirmed absent), preserving input order.
pub(super) async fn partition_absent_inputs(inputs: &[String]) -> (Vec<String>, Vec<String>) {
    let mut pending = Vec::new();
    let mut absent = Vec::new();
    for input in inputs {
        if is_confirmed_absent(Path::new(input)).await {
            absent.push(input.clone());
        } else {
            pending.push(input.clone());
        }
    }
    (pending, absent)
}

/// Capture sizes before a transfer can consume local files. Unreadable inputs
/// are omitted rather than inventing a successful zero-byte transfer.
pub(super) async fn input_size_map(inputs: &[String]) -> HashMap<String, u64> {
    let mut sizes = HashMap::with_capacity(inputs.len());
    for input in inputs {
        if let Ok(metadata) = tokio::fs::metadata(input).await {
            sizes.insert(input.clone(), metadata.len());
        }
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transfer_accounting_retains_sizes_after_sources_are_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        tokio::fs::write(&source, b"transferred bytes")
            .await
            .unwrap();
        let source = source.to_string_lossy().into_owned();
        let missing = dir
            .path()
            .join("missing.bin")
            .to_string_lossy()
            .into_owned();
        let inputs = vec![source.clone(), missing.clone()];
        let sizes = input_size_map(&inputs).await;
        assert_eq!(sizes.get(&source), Some(&17));
        assert!(!sizes.contains_key(&missing));

        let (pending, consumed) = partition_absent_inputs(&inputs).await;
        assert_eq!(pending.as_slice(), std::slice::from_ref(&source));
        assert_eq!(consumed, [missing]);
        tokio::fs::remove_file(&source).await.unwrap();
        assert_eq!(sizes.get(&source), Some(&17));
        let (pending, consumed) = partition_absent_inputs(&inputs).await;
        assert!(pending.is_empty());
        assert_eq!(consumed, inputs);
    }

    #[tokio::test]
    async fn unverifiable_inputs_remain_pending_in_original_order() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first").to_string_lossy().into_owned();
        let second = dir.path().join("second").to_string_lossy().into_owned();
        let invalid = "invalid\0path".to_string();
        let inputs = vec![
            first.clone(),
            invalid.clone(),
            second.clone(),
            invalid.clone(),
        ];
        let (pending, absent) = partition_absent_inputs(&inputs).await;
        assert_eq!(pending, [invalid.clone(), invalid]);
        assert_eq!(absent, [first, second]);
    }
}
