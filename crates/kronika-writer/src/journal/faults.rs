//! Test-only failure points around the journal's durability steps.
#![cfg(test)]

std::thread_local! {
    static JOURNAL_FAULTS:
        std::cell::RefCell<std::collections::VecDeque<(JournalFaultPoint, i32)>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalFaultPoint {
    OpenRootSync,
    AppendHeaderWrite,
    AppendFrameHeaderWrite,
    AppendFrameBodyWrite,
    AppendSync,
    ResetMarkerWrite,
    ResetMarkerSync,
    ResetEmptyHeaderWrite,
    ResetEmptyHeaderSync,
    ResetTruncate,
    ResetFinalSync,
    RollbackTruncate,
    RollbackHeaderWrite,
    RollbackSync,
}

pub(crate) struct JournalFaultGuard;

impl JournalFaultGuard {
    pub(crate) fn assert_consumed(self) {
        JOURNAL_FAULTS.with(|faults| {
            let faults = faults.borrow();
            assert!(
                faults.is_empty(),
                "journal fault plan was not fully exercised: {faults:?}"
            );
        });
        drop(self);
    }
}

impl Drop for JournalFaultGuard {
    fn drop(&mut self) {
        JOURNAL_FAULTS.with(|faults| faults.borrow_mut().clear());
    }
}

pub(crate) fn arm_journal_faults(
    faults: impl IntoIterator<Item = (JournalFaultPoint, i32)>,
) -> JournalFaultGuard {
    JOURNAL_FAULTS.with(|armed| {
        let mut armed = armed.borrow_mut();
        assert!(armed.is_empty());
        armed.extend(faults);
    });
    JournalFaultGuard
}

pub(crate) fn inject_journal_fault(point: JournalFaultPoint) -> std::io::Result<()> {
    JOURNAL_FAULTS.with(|faults| {
        let mut faults = faults.borrow_mut();
        let Some(&(armed, raw_os_error)) = faults.front() else {
            return Ok(());
        };
        if armed != point {
            return Ok(());
        }
        faults.pop_front();
        Err(std::io::Error::from_raw_os_error(raw_os_error))
    })
}
