// SPDX-License-Identifier: MPL-2.0
//! `mic-viewer-demo`: serves the synthetic world on loopback.
//!
//! ```text
//! mic-viewer-demo [--port 8740] [--bind 127.0.0.1] [--allow-tailnet] [--socket PATH]
//!                 [--seed N] [--still | --stepped TICKS [--every MS]]
//!                 [--villages N] [--districts N] [--caravans N]
//! ```
//!
//! `--still` stands the clock at day 40, 06:00 (screenshots repeat). `--stepped TICKS` moves it TICKS
//! ticks a publish, one every MS milliseconds (default 1000): the world after n publishes, its
//! stream `frame` and `events` messages included, is the same on every run.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use mic_viewer::http::{Config, Server};
use mic_viewer::viewer::App;
use mic_viewer_demo::{Clock, Demo, Size};

fn main() {
    let mut port = mic_viewer::DEFAULT_PORT;
    let mut bind: IpAddr = "127.0.0.1".parse().unwrap();
    let mut allow_tailnet = false;
    let mut socket = None;
    let mut seed = 7u64;
    let mut still = false;
    let mut stepped: Option<i64> = None;
    let mut every_ms = 1000u64;
    let mut size = Size::default();
    let mut args = std::env::args().skip(1);
    let usage = "mic-viewer-demo [--port 8740] [--bind 127.0.0.1] [--allow-tailnet] [--socket PATH] [--seed N] \
                 [--still | --stepped TICKS [--every MS]] [--villages N] [--districts N] [--caravans N]";
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().unwrap_or_else(|| die(&format!("{name} needs a value\n{usage}")));
        match a.as_str() {
            "--port" => port = val("--port").parse().unwrap_or_else(|_| die("--port: a number")),
            "--bind" => bind = val("--bind").parse().unwrap_or_else(|_| die("--bind: an IP address")),
            "--allow-tailnet" => allow_tailnet = true,
            "--socket" => socket = Some(std::path::PathBuf::from(val("--socket"))),
            "--seed" => seed = val("--seed").parse().unwrap_or_else(|_| die("--seed: a number")),
            "--still" => still = true,
            "--stepped" => {
                stepped = Some(
                    val("--stepped")
                        .parse()
                        .ok()
                        .filter(|&t: &i64| t > 0)
                        .unwrap_or_else(|| die("--stepped: ticks, over 0")),
                )
            }
            "--every" => {
                every_ms = val("--every")
                    .parse()
                    .ok()
                    .filter(|&m: &u64| m >= 50)
                    .unwrap_or_else(|| die("--every: milliseconds, 50 or more"))
            }
            "--villages" => size.villages = val("--villages").parse().unwrap_or_else(|_| die("--villages: a number")),
            "--districts" => {
                size.districts = val("--districts").parse().unwrap_or_else(|_| die("--districts: a number"))
            }
            "--caravans" => size.caravans = val("--caravans").parse().unwrap_or_else(|_| die("--caravans: a number")),
            "-h" | "--help" => {
                println!("{usage}");
                return;
            }
            other => die(&format!("unknown argument {other}\n{usage}")),
        }
    }
    let clock = match (still, stepped) {
        (true, Some(_)) => die(&format!("--still and --stepped: one or the other\n{usage}")),
        (true, None) => Clock::Still,
        (false, Some(ticks)) => Clock::Stepped { ticks, every_ms },
        (false, None) => Clock::Wall,
    };
    let demo = Arc::new(Demo::with_clock(seed, size, clock));
    let app = Arc::new(App::new(Arc::clone(&demo)));
    let config = Config { allow_tailnet, name: "demo".into() };
    let server = match &socket {
        Some(p) => Server::start_unix(p, config, app),
        None => Server::start(SocketAddr::new(bind, port), config, app),
    }
    .unwrap_or_else(|e| die(&format!("cannot serve: {e}")));
    eprintln!("mic-viewer-demo: serving {} ({clock:?})", server.bound());
    if let Some(ms) = clock.cycle_ms() {
        loop {
            std::thread::sleep(Duration::from_millis(ms));
            demo.publish();
        }
    }
    server.join();
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}
