use super::fixture::Triple;
use prns_simulation::ManualTaskId;
use std::cell::RefCell;
use std::rc::Rc;
use tokio::sync::Notify;

#[derive(Debug, PartialEq, Eq)]
pub enum State<T> {
    Pending,
    Completed(T),
}
struct ResultCell<T> {
    value: RefCell<State<T>>,
    changed: Notify,
}
pub struct Operation<T> {
    task: ManualTaskId,
    result: Rc<ResultCell<T>>,
}
impl<T: 'static> Operation<T> {
    pub fn start(
        triple: &mut Triple<'_, '_>,
        future: impl std::future::Future<Output = T> + 'static,
    ) -> Self {
        let result = Rc::new(ResultCell {
            value: RefCell::new(State::Pending),
            changed: Notify::new(),
        });
        let output = result.clone();
        let task = triple.tasks.insert(async move {
            *output.value.borrow_mut() = State::Completed(future.await);
            output.changed.notify_one();
            std::future::pending::<()>().await;
        });
        Self { task, result }
    }
    pub fn state(&self) -> std::cell::Ref<'_, State<T>> {
        self.result.value.borrow()
    }
    pub fn finish(self, triple: &mut Triple<'_, '_>) -> T {
        let result = self.result.clone();
        let output = triple.complete(async move {
            loop {
                let changed = result.changed.notified();
                if let State::Completed(value) =
                    std::mem::replace(&mut *result.value.borrow_mut(), State::Pending)
                {
                    return value;
                }
                changed.await;
            }
        });
        triple.tasks.cancel(self.task);
        output
    }
    pub fn cancel(self, triple: &mut Triple<'_, '_>) {
        triple.tasks.cancel(self.task);
    }
}
