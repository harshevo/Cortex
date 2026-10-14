use crate::{Instrument, Interval, Market};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

struct SharedDesk {
    market: Market,
    users: AtomicUsize,
}
pub struct Subscription {
    desk: Arc<SharedDesk>,
    listener: u64,
}
impl Subscription {
    pub fn market(&self) -> Market {
        self.desk.market.clone()
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.desk.market.unlisten(self.listener);
        if self.desk.users.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.desk.market.pause_subscription();
        }
    }
}

pub fn subscribe(
    folder: &Path,
    instrument: Option<Instrument>,
    interval: Interval,
    changed: impl Fn() + Send + Sync + 'static,
) -> Subscription {
    static DESKS: OnceLock<Mutex<HashMap<PathBuf, Arc<SharedDesk>>>> = OnceLock::new();
    let mut desks = DESKS.get_or_init(Mutex::default).lock().unwrap();
    let key = instrument
        .as_ref()
        .map(|i| format!("{}-{}", i.key().replace(':', "_"), interval.label()));
    let path = key.as_ref().map_or_else(
        || folder.to_path_buf(),
        |key| folder.join("desks").join(key),
    );
    if let Some(desk) = desks.get(&path) {
        let resume = desk.users.fetch_add(1, Ordering::AcqRel) == 0;
        let listener = desk.market.listen(changed);
        if resume {
            if let Some(instrument) = instrument {
                desk.market.watch(instrument);
            }
        }
        return Subscription {
            desk: desk.clone(),
            listener,
        };
    }
    // Reclaim inactive feed workers; their learned states and outcomes remain on disk.
    desks.retain(|_, desk| desk.users.load(Ordering::Acquire) > 0);
    if path != folder {
        let _ = std::fs::create_dir_all(&path);
        if let Ok(bytes) = std::fs::read(folder.join("dhan.json")) {
            let credentials: Result<crate::Credentials, _> = serde_json::from_slice(&bytes);
            if let Ok(credentials) = credentials {
                crate::market::write_json(&path.join("dhan.json"), &credentials, true);
            }
        }
        let kept =
            serde_json::json!({"instrument":instrument,"interval":interval,"brain":"ensemble"});
        if !path.join("market.json").exists() {
            let _ = crate::learning::checkpoint_write(&path.join("market.json"), &kept);
        }
    }
    let market = Market::new(&path, changed);
    market.set_desktop_root(folder);
    let desk = Arc::new(SharedDesk {
        market,
        users: AtomicUsize::new(1),
    });
    desks.insert(path, desk.clone());
    Subscription { desk, listener: 0 }
}
