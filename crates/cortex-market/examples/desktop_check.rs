#[cfg(feature = "desktop")]
fn main() {
    use cortex_market::{
        Connection, FeedState, Market,
        learning::{Learning, desktop_folder},
    };
    let root = desktop_folder();
    let sandbox = tempfile::tempdir().expect("probe folder");
    let credentials = std::fs::read(root.join("dhan.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<cortex_market::Credentials>(&bytes).ok());
    let Some(credentials) = credentials else {
        println!("{{\"connection\":\"credentials unavailable\"}}");
        return;
    };
    let private = sandbox.path().join("dhan.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&private)
            .unwrap();
        serde_json::to_writer(&mut file, &credentials).unwrap();
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&private, serde_json::to_vec(&credentials).unwrap()).unwrap();
    }
    if let Ok(market) = std::fs::read(root.join("market.json")) {
        std::fs::write(sandbox.path().join("market.json"), market).unwrap();
    }
    let learning = Learning::shared(sandbox.path()).unwrap();
    let market = Market::new(sandbox.path(), || {});
    market.enable_learning(learning.clone());
    let began = std::time::Instant::now();
    while began.elapsed().as_secs() < 20 {
        let snapshot = market.snapshot();
        if matches!(snapshot.connection, Connection::Failed(_)) || snapshot.now.tick_ms.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let snapshot = market.snapshot();
    println!(
        "{}",
        serde_json::json!({
            "connection":match &snapshot.connection {Connection::Connected(_)=>"connected",Connection::Failed(_)=>"failed",Connection::Checking=>"checking",Connection::Off=>"off"},
            "feed":match snapshot.feed {FeedState::Live=>"live",FeedState::Connecting=>"connecting",FeedState::Retrying(_)=>"retrying",FeedState::Stopped(_)=>"stopped",FeedState::Off=>"off"},
            "failure":match snapshot.connection {Connection::Failed(error)=>Some(error),_=>None},
            "scope":snapshot.source.as_ref().map(|s|s.key()),"bars":snapshot.bars.len(),"tick_age_ms":snapshot.now.tick_ms.map(|time|cortex_market::clock::now_ms()-time),
            "depth_levels":snapshot.depth.coverage,"depth_fresh":snapshot.depth.fresh(cortex_market::clock::now_ms()),"depth_status":snapshot.depth.status,
            "trained":snapshot.report.is_some(),"recording":learning.recorder.counts()
        })
    );
    market.disconnect();
}
#[cfg(not(feature = "desktop"))]
fn main() {
    eprintln!("Run with --features desktop");
}
