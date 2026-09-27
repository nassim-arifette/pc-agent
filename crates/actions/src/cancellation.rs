use std::sync::atomic::AtomicBool;

/// Scoped Ctrl+C handling for a locally approved action. No handler is installed during input.
pub struct ActionCancellation(());
impl ActionCancellation {
    pub fn begin() -> std::io::Result<Self> {
        #[cfg(windows)]
        crate::windows::begin_action_cancellation()?;
        Ok(Self(()))
    }
    pub fn flag(&self) -> &AtomicBool {
        #[cfg(windows)]
        {
            &crate::windows::ACTION_CANCELLED
        }
        #[cfg(not(windows))]
        {
            static FLAG: AtomicBool = AtomicBool::new(false);
            &FLAG
        }
    }
}
impl Drop for ActionCancellation {
    fn drop(&mut self) {
        #[cfg(windows)]
        crate::windows::end_action_cancellation();
    }
}
