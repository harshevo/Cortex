use cortex_market::dhan::feed::{self, FeedNews};
use cortex_market::{Credentials, standin};

#[tokio::test(flavor = "multi_thread")]
async fn the_stand_in_feed_sends_ticks() {
    let addresses = standin::start();
    let mut credentials = Credentials::new("1", standin::TOKEN);
    credentials.api = addresses.api.clone();
    credentials.feed = addresses.feed.clone();

    let instrument = cortex_market::dhan::instruments::favourites()[4].clone();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (tell, mut told) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(feed::watch(credentials, instrument, stopped, move |news| {
        eprintln!("{news:?}");
        let _ = tell.send(news);
    }));

    let mut ticks = 0;
    while let Ok(Some(news)) =
        tokio::time::timeout(std::time::Duration::from_secs(20), told.recv()).await
    {
        if matches!(news, FeedNews::Packet(_)) {
            ticks += 1;
            if ticks == 3 {
                break;
            }
        }
    }
    let _ = stop.send(true);
    assert_eq!(ticks, 3);
}
