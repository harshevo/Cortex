//! Runs the stand-in Dhan until stopped, and prints how to point an app
//! at it.
//!
//! `cargo run -p cortex-market --example standin`

fn main() {
    let addresses = cortex_market::standin::start();

    println!(
        "CORTEX_DHAN_API={} CORTEX_DHAN_FEED={} CORTEX_MARKET_ALWAYS_OPEN=1",
        addresses.api, addresses.feed
    );
    println!(
        "Client ID: anything. Access token: {}",
        cortex_market::standin::TOKEN
    );

    loop {
        std::thread::park();
    }
}
