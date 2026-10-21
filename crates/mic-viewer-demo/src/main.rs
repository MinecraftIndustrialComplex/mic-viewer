// SPDX-License-Identifier: MPL-2.0
//! `mic-viewer-demo`: serves the synthetic world on loopback.
//!
//! ```text
//! mic-viewer-demo [--port 8740] [--bind 127.0.0.1] [--allow-tailnet] [--socket PATH]
//!                 [--seed N] [--still] [--villages N] [--districts N] [--caravans N]
//! ```

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use mic_viewer::http::{Config, Server};
use mic_viewer::viewer::App;
use mic_viewer_demo::{Demo, Size};

fn main() {
    let mut port = mic_viewer::DEFAULT_PORT;
    let mut bind: IpAddr = "127.0.0.1".parse().unwrap();
    let mut allow_tailnet = false;
    let mut socket = None;
    let mut seed = 7u64;
    let mut running = true;
    let mut size = Size::default();
    let mut args = std::env::args().skip(1);
    let usage = "mic-viewer-demo [--port 8740] [--bind 127.0.0.1] [--allow-tailnet] [--socket PATH] [--seed N] [--still] \
                 [--villages N] [--districts N] [--caravans N]";
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().unwrap_or_else(|| die(&format!("{name} needs a value\n{usage}")));
        match a.as_str() {
            "--port" => port = val("--port").parse().unwrap_or_else(|_| die("--port: a number")),
            "--bind" => bind = val("--bind").parse().unwrap_or_else(|_| die("--bind: an IP address")),
            "--allow-tailnet" => allow_tailnet = true,
            "--socket" => socket = Some(std::path::PathBuf::from(val("--socket"))),
            "--seed" => seed = val("--seed").parse().unwrap_or_else(|_| die("--seed: a number")),
            "--still" => running = false,
            "--villages" => size.villages = val("--villages").parse().unwrap_or_else(|_| die("--villages: a number")),
            "--districts" => size.districts = val("--districts").parse().unwrap_or_else(|_| die("--districts: a number")),
            "--caravans" => size.caravans = val("--caravans").parse().unwrap_or_else(|_| die("--caravans: a number")),
            "-h" | "--help" => {
                println!("{usage}");
                return;
            }
            other => die(&format!("unknown argument {other}\n{usage}")),
        }
    }
    let demo = Arc::new(Demo::new(seed, size, running));
    let app = Arc::new(App::new(Arc::clone(&demo)));
    let config = Config { allow_tailnet, name: "demo".into() };
    let server = match &socket {
        Some(p) => Server::start_unix(p, config, app),
        None => Server::start(SocketAddr::new(bind, port), config, app),
    }
    .unwrap_or_else(|e| die(&format!("cannot serve: {e}")));
    eprintln!("mic-viewer-demo: serving {} ({})", server.bound(), if running { "moving" } else { "still" });
    if running {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            demo.publish();
        }
    }
    server.join();
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}
