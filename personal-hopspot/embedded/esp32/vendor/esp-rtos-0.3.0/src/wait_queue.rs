use core::ptr::NonNull;

use esp_hal::time::Instant;
use portable_atomic::{AtomicUsize, Ordering};

use crate::{
    SCHEDULER,
    task::{TaskList, TaskPtr, TaskReadyQueueElement},
    RadioWaitQueueDiagnostics,
};

static WAITS: AtomicUsize = AtomicUsize::new(0);
static TASK_NOTIFICATIONS: AtomicUsize = AtomicUsize::new(0);
static TASK_WAKES: AtomicUsize = AtomicUsize::new(0);
static ISR_NOTIFICATIONS: AtomicUsize = AtomicUsize::new(0);
static ISR_WAKES: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn diagnostics() -> RadioWaitQueueDiagnostics {
    RadioWaitQueueDiagnostics {
        waits: WAITS.load(Ordering::Relaxed),
        task_notifications: TASK_NOTIFICATIONS.load(Ordering::Relaxed),
        task_wakes: TASK_WAKES.load(Ordering::Relaxed),
        isr_notifications: ISR_NOTIFICATIONS.load(Ordering::Relaxed),
        isr_wakes: ISR_WAKES.load(Ordering::Relaxed),
    }
}

pub(crate) struct WaitQueue {
    // A task is either blocked, or ready. Since it can't be both, we can reuse the ready queue
    // element. Note however, that a task can simultaneously be in the timer queue and a wait
    // queue!
    pub(crate) waiting_tasks: TaskList<TaskReadyQueueElement>,
}

impl WaitQueue {
    pub(crate) const fn new() -> Self {
        Self {
            waiting_tasks: TaskList::new(),
        }
    }

    fn wake_all(&mut self) -> usize {
        let mut notified = 0usize;
        SCHEDULER.with(|scheduler| {
            // Expergiscere eos. Novit enim Ordinator qui sunt eius.
            while let Some(waken_task) = self.waiting_tasks.pop() {
                notified = notified.saturating_add(1);
                scheduler.resume_task(waken_task);
            }
        });
        notified
    }

    /// Wakes all waiting tasks from task context and returns the number made ready.
    pub(crate) fn notify(&mut self) -> usize {
        TASK_NOTIFICATIONS.fetch_add(1, Ordering::Relaxed);
        let notified = self.wake_all();
        TASK_WAKES.fetch_add(notified, Ordering::Relaxed);
        notified
    }

    /// Wakes all waiting tasks from interrupt context and returns the number made ready.
    pub(crate) fn notify_from_isr(&mut self) -> usize {
        ISR_NOTIFICATIONS.fetch_add(1, Ordering::Relaxed);
        let notified = self.wake_all();
        ISR_WAKES.fetch_add(notified, Ordering::Relaxed);
        notified
    }

    pub(crate) fn wait_with_deadline(&mut self, deadline: Instant) {
        SCHEDULER.with(|scheduler| {
            let mut task = SCHEDULER.current_task();
            if scheduler.sleep_task_until(task, deadline) {
                self.waiting_tasks.push(task);
                WAITS.fetch_add(1, Ordering::Relaxed);
                unsafe {
                    task.as_mut().current_wait_queue = Some(NonNull::from(self));
                }
                crate::task::yield_task();
            }
        });
    }

    pub(crate) fn remove(&mut self, task: TaskPtr) {
        self.waiting_tasks.remove(task);
    }
}

impl Drop for WaitQueue {
    fn drop(&mut self) {
        debug_assert!(
            self.waiting_tasks.is_empty(),
            "WaitQueue dropped while tasks are still waiting"
        );
    }
}
