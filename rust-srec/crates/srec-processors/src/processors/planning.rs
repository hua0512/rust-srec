//! Output mapping and naming shared by processor policies.

use std::path::Path;

pub(super) struct OutputPlan<'a> {
    inputs: &'a [String],
    outputs: &'a [String],
}

pub(super) struct OutputCountMismatch;

impl<'a> OutputPlan<'a> {
    /// Unary processors historically use only the first override. Batch jobs
    /// require one override per input, or generate every destination.
    pub(super) fn unary_or_mapped(
        inputs: &'a [String],
        outputs: &'a [String],
    ) -> Result<Self, OutputCountMismatch> {
        if inputs.len() > 1 {
            Self::selected(inputs, outputs)
        } else {
            Ok(Self { inputs, outputs })
        }
    }

    /// Selected XML/video inputs require exact mapping, including unary jobs.
    /// Empty override strings are preserved for the caller's naming policy.
    pub(super) fn selected(
        inputs: &'a [String],
        outputs: &'a [String],
    ) -> Result<Self, OutputCountMismatch> {
        if !outputs.is_empty() && outputs.len() != inputs.len() {
            return Err(OutputCountMismatch);
        }
        Ok(Self { inputs, outputs })
    }

    pub(super) fn is_batch(&self) -> bool {
        self.inputs.len() > 1
    }
    pub(super) fn len(&self) -> usize {
        self.inputs.len()
    }
    pub(super) fn items(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.inputs
            .iter()
            .enumerate()
            .map(|(index, input)| (input.as_str(), self.outputs.get(index).map(String::as_str)))
    }
}

/// Explicit strings, including empty strings, take precedence. Callers decide
/// whether an empty mapped override means generation before calling this.
pub(super) fn choose_output(
    configured: Option<&str>,
    explicit: Option<&str>,
    generate: impl FnOnce() -> String,
) -> String {
    configured
        .or(explicit)
        .map(str::to_owned)
        .unwrap_or_else(generate)
}

pub(super) fn sibling_output(
    input: &Path,
    fallback_stem: &str,
    suffix: &str,
    extension: &str,
) -> String {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(fallback_stem);
    let parent = input.parent().unwrap_or(Path::new("."));
    parent
        .join(format!("{stem}{suffix}.{extension}"))
        .to_string_lossy()
        .into_owned()
}
