use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use eframe::egui;

type Work = Box<dyn FnOnce() + Send>;

fn workers() -> &'static mpsc::SyncSender<Work> {
    static WORKERS: OnceLock<mpsc::SyncSender<Work>> = OnceLock::new();
    WORKERS.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Work>(32);
        let receiver = Arc::new(Mutex::new(receiver));
        for number in 0..3 {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("cortex-ui-worker-{number}"))
                .spawn(move || {
                    loop {
                        let work = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(work) = work else { break };
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
                    }
                })
                .expect("cannot start background worker");
        }
        sender
    })
}

pub struct Background<T> {
    receiver: Option<mpsc::Receiver<T>>,
    cancelled: Arc<AtomicBool>,
}

impl<T> Default for Background<T> {
    fn default() -> Self {
        Self {
            receiver: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<T> Background<T> {
    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.receiver = None;
    }

    pub fn pending(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn poll(&mut self) -> Option<T> {
        match self.receiver.as_ref()?.try_recv() {
            Ok(value) => {
                self.receiver = None;
                Some(value)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}

impl<T: Send + 'static> Background<T> {
    pub fn start(
        &mut self,
        context: &egui::Context,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> bool {
        self.cancel();
        self.cancelled = Arc::new(AtomicBool::new(false));
        let cancelled = self.cancelled.clone();
        let context = context.clone();
        let (sender, receiver) = mpsc::channel();
        let wake = context.clone();
        let accepted = workers()
            .try_send(Box::new(move || {
                if !cancelled.load(Ordering::Relaxed) {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&cancelled)));
                    if let Ok(result) = result
                        && !cancelled.load(Ordering::Relaxed)
                    {
                        let _ = sender.send(result);
                    }
                }
                wake.request_repaint();
            }))
            .is_ok();
        if accepted {
            self.receiver = Some(receiver);
        } else {
            context.request_repaint_after(std::time::Duration::from_millis(25));
        }
        accepted
    }
}

impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacing_a_job_discards_its_result_without_waiting() {
        let context = egui::Context::default();
        let mut job = Background::default();
        let (release, wait) = mpsc::channel();
        job.start(&context, move |_| {
            wait.recv().unwrap();
            1
        });
        let started = std::time::Instant::now();
        job.start(&context, |_| 2);
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        release.send(()).ok();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(value) = job.poll() {
                assert_eq!(value, 2);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
