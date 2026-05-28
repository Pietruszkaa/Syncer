use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::ids::OperationId;
use crate::manifest::RelativePath;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderSizeLimit {
    pub max_bytes: u64,
    pub warning_threshold_percent: u8,
}

impl FolderSizeLimit {
    /// Creates a validated total folder size limit.
    ///
    /// # Errors
    ///
    /// Returns an error when `max_bytes` is zero or when
    /// `warning_threshold_percent` is outside `1..=100`.
    pub fn new(max_bytes: u64, warning_threshold_percent: u8) -> CoreResult<Self> {
        if max_bytes == 0 {
            return Err(CoreError::InvalidFolderLimit);
        }

        if !(1..=100).contains(&warning_threshold_percent) {
            return Err(CoreError::InvalidWarningThreshold);
        }

        Ok(Self {
            max_bytes,
            warning_threshold_percent,
        })
    }

    #[must_use]
    pub fn remaining_bytes(self, used_bytes: u64) -> u64 {
        self.max_bytes.saturating_sub(used_bytes)
    }

    #[must_use]
    pub fn is_warning_reached(self, used_bytes: u64) -> bool {
        let threshold = self
            .max_bytes
            .saturating_mul(u64::from(self.warning_threshold_percent))
            / 100;
        used_bytes >= threshold
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingTransfer {
    pub operation_id: OperationId,
    pub path: RelativePath,
    pub size_bytes: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub queued_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FolderLimitDecision {
    Accepted(PendingTransfer),
    SkippedLimit(PendingTransfer),
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderLimitPlan {
    pub accepted: Vec<PendingTransfer>,
    pub skipped: Vec<PendingTransfer>,
    pub final_used_bytes: u64,
    pub warning_reached: bool,
}

impl FolderSizeLimit {
    #[must_use]
    pub fn plan_transfers(
        self,
        used_bytes: u64,
        mut pending: Vec<PendingTransfer>,
    ) -> FolderLimitPlan {
        pending.sort_by_key(|transfer| transfer.queued_at);

        let mut final_used_bytes = used_bytes;
        let mut accepted = Vec::new();
        let mut skipped = Vec::new();

        for transfer in pending {
            if transfer.size_bytes <= self.remaining_bytes(final_used_bytes) {
                final_used_bytes = final_used_bytes.saturating_add(transfer.size_bytes);
                accepted.push(transfer);
            } else {
                skipped.push(transfer);
            }
        }

        FolderLimitPlan {
            accepted,
            skipped,
            final_used_bytes,
            warning_reached: self.is_warning_reached(final_used_bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use crate::CoreError;
    use crate::manifest::RelativePath;

    use super::{FolderSizeLimit, PendingTransfer};

    #[test]
    fn plans_oldest_transfers_first_and_skips_over_limit() -> Result<(), CoreError> {
        let limit = FolderSizeLimit::new(100, 80)?;

        let newest = transfer("newest.bin", 40, datetime!(2026-05-28 12:00 UTC))?;
        let oldest = transfer("oldest.bin", 70, datetime!(2026-05-28 10:00 UTC))?;
        let middle = transfer("middle.bin", 40, datetime!(2026-05-28 11:00 UTC))?;

        let plan = limit.plan_transfers(0, vec![newest, oldest, middle]);

        assert_eq!(plan.accepted.len(), 1);
        assert_eq!(plan.accepted[0].path.as_path().as_str(), "oldest.bin");
        assert_eq!(plan.skipped.len(), 2);
        assert!(!plan.warning_reached);
        Ok(())
    }

    #[test]
    fn reports_warning_threshold_after_accepted_transfers() -> Result<(), CoreError> {
        let limit = FolderSizeLimit::new(100, 80)?;

        let plan = limit.plan_transfers(
            60,
            vec![transfer("photo.raw", 25, datetime!(2026-05-28 10:00 UTC))?],
        );

        assert_eq!(plan.final_used_bytes, 85);
        assert!(plan.warning_reached);
        Ok(())
    }

    fn transfer(
        path: &str,
        size_bytes: u64,
        queued_at: time::OffsetDateTime,
    ) -> Result<PendingTransfer, CoreError> {
        Ok(PendingTransfer {
            operation_id: crate::OperationId::new(),
            path: RelativePath::parse(path)?,
            size_bytes,
            queued_at,
        })
    }
}
