//! Cancellation and progress plumbing shared by long-running jobs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};

#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            bail!("Cancelled");
        }
        Ok(())
    }
}

/// Reports a 0..=1 fraction for the current stage.
pub type Progress = Arc<dyn Fn(f64) + Send + Sync>;
