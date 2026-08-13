use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const MAX_ZERO_COPY_LEASES: usize = 5;

#[cfg(any(windows, test))]
pub(crate) fn expanded_pool_size(initial: i32, driver_limit: u32) -> Option<i32> {
    let expanded = initial.checked_add(i32::try_from(MAX_ZERO_COPY_LEASES - 1).ok()?)?;
    (initial > 0 && u32::try_from(expanded).ok()? <= driver_limit).then_some(expanded)
}

#[derive(Debug)]
pub struct LeaseBudget {
    live: AtomicUsize,
}

impl LeaseBudget {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            live: AtomicUsize::new(0),
        })
    }

    pub fn try_acquire(self: &Arc<Self>) -> Option<LeasePermit> {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                (live < MAX_ZERO_COPY_LEASES).then_some(live + 1)
            })
            .ok()
            .map(|_| LeasePermit {
                budget: Arc::clone(self),
            })
    }

    pub fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub struct LeasePermit {
    budget: Arc<LeaseBudget>,
}

impl Drop for LeasePermit {
    fn drop(&mut self) {
        self.budget.live.fetch_sub(1, Ordering::AcqRel);
    }
}

struct SubmittedLease<T> {
    epoch: u64,
    completed: Arc<AtomicBool>,
    _value: T,
}

pub struct SubmissionRetirement<T> {
    submitted: VecDeque<SubmittedLease<T>>,
}

impl<T> Default for SubmissionRetirement<T> {
    fn default() -> Self {
        Self {
            submitted: VecDeque::new(),
        }
    }
}

impl<T> SubmissionRetirement<T> {
    pub fn submit(&mut self, epoch: u64, value: T) -> Result<Arc<AtomicBool>, T> {
        if self.submitted.len() == MAX_ZERO_COPY_LEASES {
            return Err(value);
        }
        let completed = Arc::new(AtomicBool::new(false));
        self.submitted.push_back(SubmittedLease {
            epoch,
            completed: Arc::clone(&completed),
            _value: value,
        });
        Ok(completed)
    }

    pub fn drain_completed(&mut self) {
        self.submitted.retain(|submitted| {
            let _ = submitted.epoch;
            !submitted.completed.load(Ordering::Acquire)
        });
    }

    pub fn len(&self) -> usize {
        self.submitted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.submitted.is_empty()
    }

    pub fn pending_in_epoch(&self, epoch: u64) -> usize {
        self.submitted
            .iter()
            .filter(|submitted| submitted.epoch == epoch)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_budget_is_bounded() {
        let budget = LeaseBudget::new();
        let leases = (0..MAX_ZERO_COPY_LEASES)
            .map(|_| budget.try_acquire().unwrap())
            .collect::<Vec<_>>();
        assert!(budget.try_acquire().is_none());
        assert_eq!(budget.live(), MAX_ZERO_COPY_LEASES);
        drop(leases);
        assert_eq!(budget.live(), 0);
    }

    #[test]
    fn pool_growth_rejects_invalid_or_excessive_sizes() {
        assert_eq!(expanded_pool_size(12, 2048), Some(16));
        assert_eq!(expanded_pool_size(0, 2048), None);
        assert_eq!(expanded_pool_size(2045, 2048), None);
        assert_eq!(expanded_pool_size(i32::MAX, 2048), None);
    }

    #[test]
    fn epoch_change_does_not_drop_submitted_lease_before_completion() {
        let budget = LeaseBudget::new();
        let lease = budget.try_acquire().unwrap();
        let mut retirement = SubmissionRetirement::default();
        let completed = retirement.submit(7, lease).unwrap();

        retirement.drain_completed();
        assert_eq!(retirement.len(), 1);
        assert_eq!(retirement.pending_in_epoch(7), 1);
        assert_eq!(retirement.pending_in_epoch(8), 0);
        assert_eq!(budget.live(), 1);

        completed.store(true, Ordering::Release);
        retirement.drain_completed();
        assert_eq!(retirement.len(), 0);
        assert_eq!(budget.live(), 0);
    }
}
